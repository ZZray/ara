//! `edit` against real files through the vendored ara-edit engine, and the
//! hashline anchors `read`/`grep` hand to it (OMP `edit/index.ts`,
//! `read.ts`/`grep.ts` hashline display).

use ara_agent::{AgentTool, Concurrency, ToolOutput};
use ara_ai::{JsonObject, UserBlock};
use ara_edit::EditMode;
use ara_tools::edit::EditTool;
use ara_tools::grep::GrepTool;
use ara_tools::read::ReadTool;
use ara_tools::write::WriteTool;
use ara_tools::{ToolContext, builtin_tools};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn args(v: Value) -> JsonObject {
    v.as_object().unwrap().clone()
}

fn text(o: &ToolOutput) -> String {
    o.content
        .iter()
        .filter_map(|b| if let UserBlock::Text(t) = b { Some(t.text.clone()) } else { None })
        .collect::<Vec<_>>()
        .join("\n<<>>\n")
}

async fn run(t: &dyn AgentTool, v: Value) -> ToolOutput {
    t.execute("c", args(v), CancellationToken::new(), Arc::new(|_| {})).await.unwrap()
}

fn tag(root: &Path, rel: &str) -> String {
    ara_edit::store::file_hash(&std::fs::read_to_string(root.join(rel)).unwrap())
}

const GREET: &str = "def greet(name):\n    msg = \"Hello, \" + name\n    print(msg)\ngreet(\"world\")\n";

/// Controlled Host services, with actual temporary-file effects. This does not
/// stand in for an external ACP editor or a real language-server process.
mod fixed_host_writer {
    use super::*;
    use ara_tools::edit_file_fallback::*;
    use ara_tools::edit_host::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::Mutex;

    #[derive(Clone, Default)]
    struct Host {
        policy: EditHostPathPolicy,
        bridge_available: bool,
        bridge_capability: bool,
        reformat_bridge: bool,
        unreadable_bridge_ack: bool,
        fail_bridge: bool,
        cancel_in_bridge: Option<CancellationToken>,
        lsp_available: bool,
        enable_lsp: bool,
        options: EditLspOptions,
        options_override: Option<Arc<Mutex<EditLspOptions>>>,
        fail_flush: bool,
        diagnostics_available: bool,
        inline_diagnostics_available: bool,
        diagnostic_consumers: Arc<Mutex<Vec<(bool, bool)>>>,
        defer_diagnostics: bool,
        fail_diagnostic_queue: bool,
        diagnostics_session: Arc<EditDiagnosticsSession>,
        late: Arc<Mutex<Vec<EditDeferredDiagnosticsHandle>>>,
        file_fallbacks: Option<Arc<EditFileMutationFallbackRegistry>>,
        mutation_session_id: Option<String>,
        events: Arc<Mutex<Vec<String>>>,
        bridge_writes: Arc<Mutex<Vec<(PathBuf, String)>>>,
        pending: Arc<Mutex<BTreeMap<String, BTreeMap<PathBuf, String>>>>,
    }

    impl Host {
        fn event(&self, value: String) {
            self.events.lock().unwrap().push(value);
        }
        fn events(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }
        fn name(path: &Path) -> String {
            path.file_name().unwrap().to_string_lossy().into_owned()
        }
        fn bridge() -> Self {
            Self { bridge_available: true, bridge_capability: true, ..Self::default() }
        }
        fn lsp() -> Self {
            Self { lsp_available: true, enable_lsp: true, ..Self::default() }
        }
        fn diagnostics() -> Value {
            json!({"server":"controlled", "messages":["file:1:1 error: fixture"], "summary":"1 error", "errored":true})
        }
    }

    impl EditHostPort for Host {
        fn path_policy(&self) -> EditHostPathPolicy {
            self.policy.clone()
        }
        fn enable_lsp(&self) -> bool {
            self.enable_lsp
        }
        fn lsp_options(&self) -> EditLspOptions {
            self.options_override.as_ref().map(|options| *options.lock().unwrap()).unwrap_or(self.options)
        }
        fn client_bridge(&self) -> Option<Arc<dyn EditClientBridge>> {
            self.bridge_available.then(|| Arc::new(self.clone()) as Arc<dyn EditClientBridge>)
        }
        fn lsp_writethrough(&self) -> Option<Arc<dyn EditLspWritethrough>> {
            self.lsp_available.then(|| Arc::new(self.clone()) as Arc<dyn EditLspWritethrough>)
        }
        fn deferred_diagnostics_host(&self) -> Option<Arc<dyn EditDeferredDiagnosticsHost>> {
            self.diagnostics_available.then(|| Arc::new(self.clone()) as Arc<dyn EditDeferredDiagnosticsHost>)
        }
        fn diagnostics_host(&self) -> Option<Arc<dyn EditDeferredDiagnosticsHost>> {
            (self.diagnostics_available || self.inline_diagnostics_available)
                .then(|| Arc::new(self.clone()) as Arc<dyn EditDeferredDiagnosticsHost>)
        }
        fn file_mutation_fallbacks(&self) -> Option<Arc<EditFileMutationFallbackRegistry>> {
            self.file_fallbacks.clone()
        }
        fn file_mutation_session_id(&self) -> Option<String> {
            self.mutation_session_id.clone()
        }
        fn invalidate_after_write(&self, path: &Path) {
            self.event(format!("invalidate-write:{}", Self::name(path)));
        }
        fn invalidate_after_delete(&self, path: &Path) {
            self.event(format!("invalidate-delete:{}", Self::name(path)));
        }
        fn invalidate_after_rename(&self, source: &Path, destination: &Path) {
            self.event(format!("invalidate-rename:{}:{}", Self::name(source), Self::name(destination)));
        }
        fn bump_file_mutation_version(&self, path: &Path) {
            self.event(format!("bump:{}", Self::name(path)));
        }
    }

    impl EditDeferredDiagnosticsHost for Host {
        fn bump_file_mutation_version(&self, path: &Path) -> u64 {
            self.event(format!("bump:{}", Self::name(path)));
            self.diagnostics_session.bump_file_mutation_version(path)
        }
        fn get_file_mutation_version(&self, path: &Path) -> u64 {
            self.diagnostics_session.get_file_mutation_version(path)
        }
        fn reduce_diagnostics(&self, path: &Path, result: Value) -> std::io::Result<Value> {
            self.diagnostics_session.reduce_diagnostics(path, result)
        }
        fn queue_deferred_diagnostics(&self, entry: EditDeferredDiagnosticEntry) -> std::io::Result<()> {
            if self.fail_diagnostic_queue {
                return Err(std::io::Error::other("controlled diagnostic queue failure"));
            }
            self.diagnostics_session.queue_deferred_diagnostics(entry)
        }
    }

    #[async_trait::async_trait]
    impl EditClientBridge for Host {
        fn can_write_text_file(&self) -> bool {
            self.bridge_capability
        }
        async fn write_text_file(&self, path: &Path, content: &str) -> std::io::Result<()> {
            assert!(path.is_absolute());
            self.bridge_writes.lock().unwrap().push((path.to_owned(), content.to_owned()));
            self.event(format!("bridge:{}:{content}", Self::name(path)));
            if self.fail_bridge {
                std::fs::write(path, "partial\n")?;
                return Err(std::io::Error::other("controlled ACP failure"));
            }
            if self.unreadable_bridge_ack {
                if path.exists() {
                    std::fs::remove_file(path)?;
                }
            } else {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let persisted = if self.reformat_bridge {
                    content
                        .split_inclusive('\n')
                        .map(|line| {
                            line.strip_prefix("    ").map_or_else(|| line.to_owned(), |rest| format!("\t{rest}"))
                        })
                        .collect::<String>()
                } else {
                    content.to_owned()
                };
                std::fs::write(path, persisted)?;
            }
            if let Some(cancel) = &self.cancel_in_bridge {
                cancel.cancel();
            }
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl EditLspWritethrough for Host {
        async fn write_through(&self, request: EditLspWriteRequest<'_>) -> std::io::Result<Option<Value>> {
            self.diagnostic_consumers
                .lock()
                .unwrap()
                .push((request.get_deferred.is_some(), request.transform_diagnostics.is_some()));
            self.event(format!("lsp:{}:{:?}", Self::name(request.absolute_path), request.options));
            if Self::name(request.absolute_path) == "fail.txt" {
                return Err(std::io::Error::other("controlled LSP write failure"));
            }
            // The controlled service exercises immediate commit / deferred
            // post-processing. It implements no actual server protocol.
            if let Some(registry) = &request.file_fallbacks {
                registry
                    .write(request.absolute_path, request.content, request.mutation_session_id.as_deref())
                    .await
                    .map_err(|error| std::io::Error::new(error.original().kind(), error.to_string()))?;
            } else {
                std::fs::write(request.absolute_path, request.content)?;
            }
            if let Some(batch) = request.batch {
                self.pending
                    .lock()
                    .unwrap()
                    .entry(batch.id.clone())
                    .or_default()
                    .insert(request.absolute_path.to_owned(), request.content.to_owned());
                if batch.flush {
                    // Mirrors the fixed writer's lazy getDeferred per target;
                    // only the final request's target gets an edit handle.
                    let paths = self.pending.lock().unwrap()[&batch.id].keys().cloned().collect::<Vec<_>>();
                    let bundles = paths
                        .iter()
                        .filter_map(|path| {
                            let factory = request.get_deferred.as_ref()?;
                            let bundle = factory(path);
                            self.event(format!("deferred-lookup:{}:{}", Self::name(path), bundle.is_some()));
                            bundle
                        })
                        .collect::<Vec<_>>();
                    let raw = self.flush_batch(&batch.id, request.cwd, request.cancel).await?;
                    let result = if self.defer_diagnostics {
                        None
                    } else {
                        raw.map(|raw| match &request.transform_diagnostics {
                            Some(transform) => transform(request.absolute_path, raw),
                            None => Ok(raw),
                        })
                        .transpose()?
                    };
                    for bundle in bundles {
                        bundle.finalize(result.as_ref());
                        if self.defer_diagnostics {
                            self.late.lock().unwrap().push(bundle);
                        }
                    }
                    return Ok(result);
                }
                return Ok(None);
            }
            let bundle = request.get_deferred.as_ref().and_then(|factory| factory(request.absolute_path));
            if request.options.enable_format {
                let formatted = format!("formatted:{}", request.content);
                if let Some(registry) = &request.file_fallbacks {
                    registry
                        .write(request.absolute_path, &formatted, request.mutation_session_id.as_deref())
                        .await
                        .map_err(|error| std::io::Error::new(error.original().kind(), error.to_string()))?;
                } else {
                    std::fs::write(request.absolute_path, formatted)?;
                }
            }
            let raw = (request.options.enable_diagnostics && !self.defer_diagnostics).then(Self::diagnostics);
            let result = raw
                .map(|raw| match &request.transform_diagnostics {
                    Some(transform) => transform(request.absolute_path, raw),
                    None => Ok(raw),
                })
                .transpose()?;
            if let Some(bundle) = bundle {
                bundle.finalize(result.as_ref());
                if self.defer_diagnostics {
                    self.late.lock().unwrap().push(bundle);
                }
            }
            Ok(result)
        }

        async fn notify_watched_files(
            &self,
            _: &Path,
            changes: &[EditFileChange],
            _: Option<CancellationToken>,
        ) -> std::io::Result<()> {
            for change in changes {
                self.event(format!("watch:{:?}:{}", change.change_type, Self::name(&change.path)));
            }
            Ok(())
        }

        async fn flush_batch(
            &self,
            id: &str,
            _: &Path,
            _: Option<CancellationToken>,
        ) -> std::io::Result<Option<Value>> {
            self.event(format!("flush:{id}"));
            let pending = self.pending.lock().unwrap().remove(id).unwrap_or_default();
            if self.fail_flush {
                return Err(std::io::Error::other("controlled LSP flush failure"));
            }
            for (path, _) in pending {
                // Read live committed bytes; do not replay the retained text.
                match std::fs::read_to_string(&path) {
                    Ok(text) => self.event(format!("flush-read:{}:{text}", Self::name(&path))),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(Some(Self::diagnostics()))
        }
    }

    fn tool(root: &Path, mode: EditMode, host: &Host) -> EditTool {
        EditTool::new(ToolContext::new(root).with_edit(mode, true).with_edit_host(Arc::new(host.clone())))
    }

    async fn with_batch(tool: &EditTool, args: Value, id: &str, flush: bool) -> ToolOutput {
        tool.execute_os_args_json_with_request(
            args.to_string(),
            ara_edit::ApplyRequest { lsp_batch_id: Some(id.into()), lsp_flush: flush },
            CancellationToken::new(),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn fixed_five_acp_plan_rows_and_settlement_family() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        for (mode, filename, original, args, expected) in [
            (
                EditMode::Replace,
                "replace.txt",
                "old content\n",
                json!({"path":"replace.txt","old_string":"old content","new_string":"new content"}),
                "new content\n",
            ),
            (
                EditMode::Patch,
                "patch.txt",
                "a\n",
                json!({"path":"patch.txt","edits":[{"op":"update","diff":"@@\n-a\n+b"}]}),
                "b\n",
            ),
        ] {
            std::fs::write(root.join(filename), original).unwrap();
            let host = Host::bridge();
            let output = run(&tool(root, mode, &host), args).await;
            assert!(!output.is_error, "{}", text(&output));
            assert_eq!(std::fs::read_to_string(root.join(filename)).unwrap(), expected);
            assert_eq!(output.details.as_ref().unwrap()["newText"], expected);
            assert_eq!(
                host.events(),
                [
                    format!("bridge:{filename}:{expected}"),
                    format!("invalidate-write:{filename}"),
                    format!("bump:{filename}")
                ]
            );
        }

        let sandbox = root.join("artifacts/local");
        std::fs::create_dir_all(&sandbox).unwrap();
        std::fs::write(sandbox.join("PLAN.md"), "old plan\n").unwrap();
        let mut host = Host::bridge();
        host.policy = EditHostPathPolicy {
            local_sandbox_root: Some(sandbox.clone()),
            plan_active: true,
            plan_file_path: Some("local://PLAN.md".into()),
            ..Default::default()
        };
        for (path, before, after) in [
            ("local://PLAN.md".to_owned(), "old plan", "new plan"),
            (sandbox.join("PLAN.md").to_string_lossy().into_owned(), "new plan", "absolute plan"),
        ] {
            let output =
                run(&tool(root, EditMode::Replace, &host), json!({"path":path,"old_string":before,"new_string":after}))
                    .await;
            assert!(!output.is_error, "{}", text(&output));
            assert_eq!(std::fs::read_to_string(sandbox.join("PLAN.md")).unwrap(), format!("{after}\n"));
        }
        assert!(!host.events().iter().any(|event| event.starts_with("bridge:")));
        std::fs::write(root.join("readonly.txt"), "old\n").unwrap();
        let denied = run(
            &tool(root, EditMode::Replace, &host),
            json!({"path":"readonly.txt","old_string":"old","new_string":"changed"}),
        )
        .await;
        assert!(denied.is_error && text(&denied).contains("working tree is read-only"));
        assert_eq!(std::fs::read_to_string(root.join("readonly.txt")).unwrap(), "old\n");

        for (filename, original, replacement, persisted, formatted) in [
            (
                "drift.ts",
                "function f() {\n    return 1;\n}\n",
                "    return 2;",
                "function f() {\n\treturn 2;\n}\n",
                true,
            ),
            ("exact.txt", "hello\nworld\n", "earth", "hello\nearth\n", false),
        ] {
            std::fs::write(root.join(filename), original).unwrap();
            let mut host = Host::bridge();
            host.reformat_bridge = formatted;
            let context = ToolContext::new(root).with_edit(EditMode::Hashline, true).with_edit_host(Arc::new(host));
            let current_tag = context.edit_store.record(&root.join(filename), original, None);
            let output = run(
                &EditTool::new(context.clone()),
                json!({"input":format!("[{filename}#{current_tag}]\nPUT 2-2:\n+{replacement}")}),
            )
            .await;
            assert!(!output.is_error, "{}", text(&output));
            assert_eq!(std::fs::read_to_string(root.join(filename)).unwrap(), persisted);
            assert_eq!(output.details.as_ref().unwrap()["newText"], persisted);
            assert_eq!(text(&output).contains("reformatted it on save"), formatted);
            assert!(text(&output).contains(&format!("#{}]", ara_edit::store::file_hash(persisted))));
            // Use the returned persisted tag in the next real edit, proving
            // the existing shared EditStore was not poisoned by requested text.
            let next_tool = EditTool::new(context);
            let next_payload = json!({"input":format!("[{filename}#{}]\nPUT 2-2:\n+{}", ara_edit::store::file_hash(persisted), if formatted { "\treturn 3;" } else { "updated" })});
            let next = run(&next_tool, next_payload.clone()).await;
            if formatted {
                // b91 session.rs carries unchanged seen rows; the previewed
                // changed row is not provenance for the formatter's new text.
                assert!(next.is_error && text(&next).contains("never displayed"), "{}", text(&next));
                assert!(text(&next).contains("2:\treturn 2;"), "{}", text(&next));
                assert_eq!(std::fs::read_to_string(root.join(filename)).unwrap(), persisted);
                let retry = run(&next_tool, next_payload).await;
                assert!(!retry.is_error, "{}", text(&retry));
                assert_eq!(std::fs::read_to_string(root.join(filename)).unwrap(), "function f() {\n\treturn 3;\n}\n");
            } else {
                assert!(!next.is_error, "{}", text(&next));
                assert_eq!(std::fs::read_to_string(root.join(filename)).unwrap(), "hello\nupdated\n");
            }
        }

        std::fs::write(root.join("cancel.txt"), "old\n").unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let host = Host::bridge();
        let output = tool(root, EditMode::Replace, &host)
            .execute_os_args_json(
                json!({"path":"cancel.txt","old_string":"old","new_string":"new"}).to_string(),
                cancel,
            )
            .await
            .unwrap();
        assert!(output.is_error);
        assert!(host.events().is_empty());
        let cancel = CancellationToken::new();
        let mut host = Host::bridge();
        host.cancel_in_bridge = Some(cancel.clone());
        let output = tool(root, EditMode::Replace, &host)
            .execute_os_args_json(
                json!({"path":"cancel.txt","old_string":"old","new_string":"new"}).to_string(),
                cancel.clone(),
            )
            .await
            .unwrap();
        assert!(!output.is_error, "{}", text(&output));
        assert!(cancel.is_cancelled());
        assert_eq!(std::fs::read_to_string(root.join("cancel.txt")).unwrap(), "new\n");
        assert_eq!(host.events().iter().filter(|event| event.starts_with("bridge:")).count(), 1);

        std::fs::write(root.join("partial.txt"), "old\n").unwrap();
        let mut host = Host::bridge();
        host.fail_bridge = true;
        let output = run(
            &tool(root, EditMode::Replace, &host),
            json!({"path":"partial.txt","old_string":"old","new_string":"new"}),
        )
        .await;
        assert!(output.is_error && text(&output).contains("persisted outcome unknown"));
        assert_eq!(std::fs::read_to_string(root.join("partial.txt")).unwrap(), "partial\n");
        assert_eq!(host.events().iter().filter(|event| event.starts_with("bridge:")).count(), 1);

        std::fs::write(root.join("readback.txt"), "old\n").unwrap();
        let mut host = Host::bridge();
        host.unreadable_bridge_ack = true;
        let output = run(
            &tool(root, EditMode::Replace, &host),
            json!({"path":"readback.txt","old_string":"old","new_string":"new"}),
        )
        .await;
        assert!(!output.is_error, "{}", text(&output));
        assert_eq!(output.details.as_ref().unwrap()["newText"], "new\n");
        assert!(!root.join("readback.txt").exists());

        std::fs::write(root.join("capability.txt"), "old\n").unwrap();
        let mut host = Host::bridge();
        host.bridge_capability = false;
        let output = run(
            &tool(root, EditMode::Replace, &host),
            json!({"path":"capability.txt","old_string":"old","new_string":"new"}),
        )
        .await;
        assert!(!output.is_error);
        assert!(!host.events().iter().any(|event| event.starts_with("bridge:")));
        assert_eq!(std::fs::read_to_string(root.join("capability.txt")).unwrap(), "new\n");

        // WRITE consumes the same real Host port, guard and ACP settlement.
        // B-4246e9769c: fixed original absolute output.txt and exact payload;
        // the controlled Host is the destination, never the native primitive.
        let mut plain_bridge = Host::bridge();
        let plain_bridge_io = Arc::new(FaultIo::default());
        plain_bridge.file_fallbacks = Some(EditFileMutationFallbackRegistry::new(plain_bridge_io.clone(), Arc::new(|_| {})));
        let plain_write = WriteTool::new(ToolContext::new(root).with_edit_host(Arc::new(plain_bridge.clone())));
        let plain_path = root.join("output.txt");
        plain_write.execute_os_values(plain_path.to_str().unwrap(), "bridge write content\n", CancellationToken::new()).await.unwrap();
        assert_eq!(*plain_bridge.bridge_writes.lock().unwrap(), vec![(plain_path, "bridge write content\n".into())]);
        assert_eq!(plain_bridge_io.count("write"), 0, "ACP owns the write; the native primitive is not called");
        let mut shared_bridge = Host::bridge();
        shared_bridge.reformat_bridge = true;
        let bridge_local_io = Arc::new(FaultIo::default());
        shared_bridge.file_fallbacks = Some(EditFileMutationFallbackRegistry::new(bridge_local_io.clone(), Arc::new(|_| {})));
        let write = WriteTool::new(
            ToolContext::new(root).with_edit(EditMode::Hashline, true).with_edit_host(Arc::new(shared_bridge.clone())),
        );
        // B-156701b5cb: original formatted.ts function and full header.
        let requested = "function f() {\n    return 1;\n}\n";
        let persisted_expected = "function f() {\n\treturn 1;\n}\n";
        let formatted_path = root.join("formatted.ts");
        let output =
            write.execute_os_values(formatted_path.to_str().unwrap(), requested, CancellationToken::new()).await.unwrap();
        let persisted = std::fs::read_to_string(&formatted_path).unwrap();
        assert_eq!(persisted, persisted_expected);
        assert!(text(&output).contains(&format!("[formatted.ts#{}]", ara_edit::store::file_hash(&persisted))));
        assert!(!text(&output).contains(&format!("[formatted.ts#{}]", ara_edit::store::file_hash(requested))));
        assert_eq!(shared_bridge.events().iter().filter(|event| event.starts_with("bridge:")).count(), 1);
        assert_eq!(bridge_local_io.count("write"), 0, "ACP owns the write; the native primitive is not called");
        let mut failing = Host::bridge();
        failing.fail_bridge = true;
        let write = WriteTool::new(ToolContext::new(root).with_edit_host(Arc::new(failing.clone())));
        let error =
            write.execute_os_values("shared-partial.txt", "requested", CancellationToken::new()).await.unwrap_err();
        assert!(error.0.contains("persisted outcome unknown"));
        assert_eq!(std::fs::read_to_string(root.join("shared-partial.txt")).unwrap(), "partial\n");
        assert_eq!(failing.events().iter().filter(|event| event.starts_with("bridge:")).count(), 1);
        let local = root.join("shared-local");
        std::fs::create_dir_all(&local).unwrap();
        let mut planning = Host::bridge();
        planning.policy = EditHostPathPolicy {
            local_sandbox_root: Some(local.clone()),
            plan_active: true,
            plan_file_path: Some("local://PLAN.md".into()),
            ..Default::default()
        };
        let write = WriteTool::new(ToolContext::new(root).with_edit_host(Arc::new(planning.clone())));
        // B-bac5b5758d: original local plan payload, active unrelated plan
        // also remains in place for B-514b028e81 bracketed scratch.
        let plan_content = "# Plan\n\nhello world\n";
        write.execute_os_values("local://PLAN.md", plan_content, CancellationToken::new()).await.unwrap();
        assert_eq!(std::fs::read_to_string(local.join("PLAN.md")).unwrap(), plan_content);
        write.execute_os_values("[local://scratch.md#ABCD]", "scratch notes\n", CancellationToken::new()).await.unwrap();
        assert_eq!(std::fs::read_to_string(local.join("scratch.md")).unwrap(), "scratch notes\n");
        assert!(!planning.events().iter().any(|event| event.starts_with("bridge:")));
        // B-4497a5b280: fixed original default Session, no ACP/plan Host.
        let default_write = WriteTool::new(ToolContext::new(root));
        let memory = default_write.execute_os_values("memory://root/memory_summary.md", "memory summary\n", CancellationToken::new()).await.unwrap_err();
        assert!(memory.0.contains("memory:// URLs are read-only for write"), "{}", memory.0);
        assert!(!root.join("memory:").exists());
        assert!(
            write
                .execute_os_values("working-tree.txt", "refused", CancellationToken::new())
                .await
                .unwrap_err()
                .0
                .contains("read-only")
        );
        assert!(!root.join("working-tree.txt").exists());

        // Native filesystem URLs use the same peeled target and policy owner;
        // they never reach ACP, even after an acknowledged local write.
        write.execute_os_values("local://draft.md:raw", "draft", CancellationToken::new()).await.unwrap();
        assert_eq!(std::fs::read_to_string(local.join("draft.md")).unwrap(), "draft");
        assert!(!local.join("draft.md:raw").exists());
        assert!(!planning.events().iter().any(|event| event.starts_with("bridge:")));
        let mut native = Host::bridge();
        native.policy.local_sandbox_root = Some(local.clone());
        let vault = root.join("shared-vault");
        std::fs::create_dir_all(&vault).unwrap();
        native.policy.vault_roots = Some(vec![("_".into(), vault.clone())]);
        let native_write = WriteTool::new(ToolContext::new(root).with_edit_host(Arc::new(native.clone())));
        native_write
            .execute_os_values("vault://_/draft.md:conflicts", "vault draft", CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(vault.join("draft.md")).unwrap(), "vault draft");
        assert!(!vault.join("draft.md:conflicts").exists());
        for path in ["local://../outside.txt", "vault://_/../outside.txt"] {
            assert!(
                native_write
                    .execute_os_values(path, "refused", CancellationToken::new())
                    .await
                    .unwrap_err()
                    .0
                    .contains("Path traversal")
            );
        }
        assert!(
            native_write
                .execute_os_values("vault://absent/draft.md", "refused", CancellationToken::new())
                .await
                .unwrap_err()
                .0
                .contains("cached vault root")
        );
        for path in ["local://draft.md:1-2", "vault://_/draft.md:1-2"] {
            assert!(
                native_write
                    .execute_os_values(path, "refused", CancellationToken::new())
                    .await
                    .unwrap_err()
                    .0
                    .contains("trailing selector")
            );
        }
        assert_eq!(
            native_write
                .execute_os_values("unknown://draft.md", "refused", CancellationToken::new())
                .await
                .unwrap_err()
                .0,
            "Unknown URI-like write target 'unknown://draft.md'. Tool devices use 'xd://<tool>'. Prefix the path with './' to write it as a filesystem path."
        );
        assert!(!root.join("outside.txt").exists());
        assert!(!native.events().iter().any(|event| event.starts_with("bridge:")));
        let absent = WriteTool::new(ToolContext::new(root));
        assert!(
            absent
                .execute_os_values("local://draft.md", "refused", CancellationToken::new())
                .await
                .unwrap_err()
                .0
                .contains("unavailable")
        );
        assert!(
            absent
                .execute_os_values("vault://_/draft.md", "refused", CancellationToken::new())
                .await
                .unwrap_err()
                .0
                .contains("disabled")
        );
    }

    #[tokio::test]
    async fn fixed_lsp_write_mutation_batch_and_failure_boundary_family() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::write(root.join("format.txt"), "old\n").unwrap();
        let mut host = Host::lsp();
        host.options = EditLspOptions { enable_format: true, enable_diagnostics: true, deduplicate_diagnostics: true };
        let output = run(
            &tool(root, EditMode::Replace, &host),
            json!({"path":"format.txt","old_string":"old","new_string":"new"}),
        )
        .await;
        assert!(!output.is_error, "{}", text(&output));
        assert_eq!(std::fs::read_to_string(root.join("format.txt")).unwrap(), "formatted:new\n");
        assert_eq!(output.details.as_ref().unwrap()["newText"], "new\n"); // fixed LSP is requested content
        assert_eq!(output.details.as_ref().unwrap()["diagnostics"]["errored"], true);
        assert!(
            host.events()[0].contains("enable_format: true, enable_diagnostics: true, deduplicate_diagnostics: true")
        );
        assert_eq!(&host.events()[1..], ["invalidate-write:format.txt", "bump:format.txt"]);

        let mut disabled = Host::lsp();
        disabled.enable_lsp = false;
        disabled.options = host.options;
        let output = run(
            &tool(root, EditMode::Patch, &disabled),
            json!({"path":"disabled.txt","edits":[{"op":"create","diff":"plain\n"}]}),
        )
        .await;
        assert!(!output.is_error);
        assert!(!disabled.events().iter().any(|event| event.starts_with("lsp:")));
        assert_eq!(std::fs::read_to_string(root.join("disabled.txt")).unwrap(), "plain\n");

        let mutation = Host::lsp();
        let output = with_batch(
            &tool(root, EditMode::Patch, &mutation),
            json!({"path":"source.txt","edits":[{"op":"create","diff":"a\n"}]}),
            "mutations",
            false,
        )
        .await;
        assert!(!output.is_error);
        assert_eq!(std::fs::read_to_string(root.join("source.txt")).unwrap(), "a\n");
        let output = with_batch(
            &tool(root, EditMode::Patch, &mutation),
            json!({"path":"source.txt","edits":[{"op":"update","rename":"destination.txt","diff":"@@\n-a\n+b"}]}),
            "mutations",
            true,
        )
        .await;
        assert!(!output.is_error, "{}", text(&output));
        assert!(!root.join("source.txt").exists());
        assert_eq!(std::fs::read_to_string(root.join("destination.txt")).unwrap(), "b\n");
        assert!(mutation.events().windows(6).any(|events| events
            == [
                "watch:Deleted:source.txt",
                "watch:Created:destination.txt",
                "invalidate-rename:source.txt:destination.txt",
                "bump:source.txt",
                "bump:destination.txt",
                "flush:mutations"
            ]));
        let output = with_batch(
            &tool(root, EditMode::Patch, &mutation),
            json!({"path":"destination.txt","edits":[{"op":"delete"}]}),
            "delete",
            true,
        )
        .await;
        assert!(!output.is_error);
        assert!(!root.join("destination.txt").exists());
        assert!(mutation.events().windows(4).any(|events| events
            == [
                "watch:Deleted:destination.txt",
                "invalidate-delete:destination.txt",
                "bump:destination.txt",
                "flush:delete"
            ]));

        let batch = Host::lsp();
        let editor = tool(root, EditMode::Patch, &batch);
        let output =
            with_batch(&editor, json!({"path":"batch.txt","edits":[{"op":"create","diff":"first\n"}]}), "batch", false)
                .await;
        assert!(!output.is_error);
        assert_eq!(std::fs::read_to_string(root.join("batch.txt")).unwrap(), "first\n");
        std::fs::write(root.join("batch.txt"), "external latest\n").unwrap();
        let failed =
            with_batch(&editor, json!({"path":"fail.txt","edits":[{"op":"create","diff":"fail\n"}]}), "batch", true)
                .await;
        assert!(failed.is_error && text(&failed).contains("controlled LSP write failure"));
        assert!(batch.events().iter().any(|event| event == "flush-read:batch.txt:external latest\n"));
        assert_eq!(std::fs::read_to_string(root.join("batch.txt")).unwrap(), "external latest\n");
        assert_eq!(batch.events().iter().filter(|event| event.as_str() == "flush:batch").count(), 1);
        assert!(batch.pending.lock().unwrap().is_empty());

        let malformed =
            with_batch(&editor, json!({"path":"missing.txt","old_string":"x","new_string":"y"}), "parse-error", true)
                .await;
        assert!(malformed.is_error);
        assert!(batch.events().iter().any(|event| event == "flush:parse-error"));
        let mut failure = Host::lsp();
        failure.fail_flush = true;
        let output = with_batch(
            &tool(root, EditMode::Replace, &failure),
            json!({"path":"missing.txt","old_string":"x","new_string":"y"}),
            "flush-error",
            true,
        )
        .await;
        assert!(output.is_error && text(&output).contains("controlled LSP flush failure"));

        // Extend this existing coarse family; no new micro-test entrance.
        // These are controlled Session services, not an installed LSP process
        // or production model/transcript yield delivery.
        let mut late_host = Host::lsp();
        late_host.diagnostics_available = true;
        late_host.defer_diagnostics = true;
        late_host.options =
            EditLspOptions { enable_format: false, enable_diagnostics: true, deduplicate_diagnostics: true };
        let late_tool = tool(root, EditMode::Patch, &late_host);
        let output = run(&late_tool, json!({"path":"late.txt","edits":[{"op":"create","diff":"written now\n"}]})).await;
        assert!(!output.is_error, "{}", text(&output));
        assert_eq!(std::fs::read_to_string(root.join("late.txt")).unwrap(), "written now\n");
        let handle = late_host.late.lock().unwrap().pop().unwrap();
        assert!(!handle.signal().is_cancelled(), "a completed edit-call child must not abort the deferred fetch");
        assert_eq!(late_host.diagnostics_session.get_file_mutation_version(&root.join("late.txt")), 1);
        let diagnostic = |messages: Vec<&str>| json!({"server":"controlled", "messages":messages, "summary":"actual summary", "errored":true});
        assert!(handle.on_deferred_diagnostics(diagnostic(vec!["late.txt:1:1 [error] late first"])).unwrap());
        assert_eq!(late_host.diagnostics_session.pending_count(), 1);
        // b91db66e begin claims this mutation once; its own late result remains current.
        let own_late = late_host.diagnostics_session.drain_lazy().survivors();
        assert_eq!(own_late.len(), 1);
        assert_eq!(own_late[0].path, root.join("late.txt"));
        assert_eq!(own_late[0].messages, vec!["late.txt:1:1 [error] late first"]);
        assert_eq!(own_late[0].summary, "actual summary");
        assert!(own_late[0].errored);

        let session_host: Arc<dyn EditDeferredDiagnosticsHost> = Arc::new(late_host.clone());
        let owner = EditDeferredDiagnostics::new(session_host.clone(), true);
        let path = root.join("fresh.txt");
        let fresh = owner.begin(&path);
        fresh.finalize(None);
        assert!(fresh.on_deferred_diagnostics(diagnostic(vec!["fresh.txt:1:1 [error] fresh"])).unwrap());
        let lazy = late_host.diagnostics_session.drain_lazy();
        session_host.bump_file_mutation_version(&path); // another mutation after lazy drain
        assert!(lazy.survivors().is_empty(), "freshness is evaluated at injection");

        let aborted = owner.begin(&path);
        aborted.finalize(None);
        let replacement = owner.begin(&path);
        assert!(aborted.signal().is_cancelled());
        assert!(!aborted.on_deferred_diagnostics(diagnostic(vec!["fresh.txt:2:3 [error] cancelled"])).unwrap());
        replacement.finalize(Some(&Host::diagnostics()));
        assert!(replacement.signal().is_cancelled());
        assert!(!replacement.on_deferred_diagnostics(Host::diagnostics()).unwrap());

        let first = owner.begin(&root.join("independent-a.txt"));
        first.finalize(None);
        let other = owner.begin(&root.join("independent-b.txt"));
        assert!(!first.signal().is_cancelled());
        other.finalize(Some(&Host::diagnostics()));
        assert!(first.on_deferred_diagnostics(diagnostic(vec!["a:1:1 [error] one", "a:1:2 [WARNING] two"])).unwrap());
        let survivors = late_host.diagnostics_session.drain_lazy().survivors();
        assert_eq!(survivors.len(), 1);
        assert_eq!(survivors[0].path, root.join("independent-a.txt"));
        assert_eq!(survivors[0].summary, "actual summary");
        assert_eq!(survivors[0].messages.len(), 2);
        assert!(survivors[0].errored);

        // Inline and late paths share the Session ledger. Moving a diagnostic
        // location does not make its same identity fresh. Empty clears history.
        let inline_path = root.join("inline.txt");
        session_host.reduce_diagnostics(&inline_path, diagnostic(vec!["inline.txt:1:1 [error] shared"])).unwrap();
        let same = owner.begin(&inline_path);
        same.finalize(None);
        assert!(!same.on_deferred_diagnostics(diagnostic(vec!["inline.txt:90:20 [error] shared"])).unwrap());
        assert_eq!(late_host.diagnostics_session.pending_count(), 0);
        session_host.reduce_diagnostics(&inline_path, diagnostic(vec![])).unwrap();
        let cleared = owner.begin(&inline_path);
        cleared.finalize(None);
        assert!(cleared.on_deferred_diagnostics(diagnostic(vec!["inline.txt:91:20 [error] shared"])).unwrap());
        assert_eq!(late_host.diagnostics_session.drain_lazy().survivors().len(), 1);
        let reduced = session_host
            .reduce_diagnostics(
                &inline_path,
                diagnostic(vec![
                    "inline.txt:92:20 [error] shared",
                    "inline.txt:2:1 [WARNING] next",
                    "inline.txt:3:1 [info] next",
                    "inline.txt:4:1 [HINT] next",
                ]),
            )
            .unwrap();
        assert_eq!(reduced["messages"].as_array().unwrap().len(), 3);
        assert_eq!(reduced["summary"], "1 warning(s), 1 info(s), 1 hint(s)");
        assert_eq!(reduced["errored"], false);
        assert!(session_host.reduce_diagnostics(&inline_path, json!({"messages":[7]})).is_err());

        let no_dedup = EditDeferredDiagnostics::new(session_host.clone(), false);
        let empty = no_dedup.begin(&root.join("empty.txt"));
        empty.finalize(None);
        assert!(empty.on_deferred_diagnostics(json!({"messages":[],"summary":null,"errored":false})).unwrap());
        let empty_entries = late_host.diagnostics_session.drain_lazy().survivors();
        assert_eq!(empty_entries.len(), 1);
        assert_eq!(empty_entries[0].summary, "");
        assert!(empty_entries[0].messages.is_empty());
        let mut queue_failure = late_host.clone();
        queue_failure.fail_diagnostic_queue = true;
        let failing =
            EditDeferredDiagnostics::new(Arc::new(queue_failure), false).begin(&root.join("queue-failure.txt"));
        assert!(
            failing
                .on_deferred_diagnostics(Host::diagnostics())
                .unwrap_err()
                .to_string()
                .contains("controlled diagnostic queue failure")
        );

        // Batched commits do not allocate handles before the flush. The fixed
        // callback supplies one only for the final write's current target.
        let mut deferred_batch = Host::lsp();
        deferred_batch.diagnostics_available = true;
        deferred_batch.defer_diagnostics = true;
        deferred_batch.options = late_host.options;
        let batch_tool = tool(root, EditMode::Patch, &deferred_batch);
        let output = with_batch(
            &batch_tool,
            json!({"path":"batch-earlier.txt","edits":[{"op":"create","diff":"first\n"}]}),
            "deferred",
            false,
        )
        .await;
        assert!(!output.is_error);
        assert!(deferred_batch.late.lock().unwrap().is_empty());
        assert_eq!(deferred_batch.diagnostics_session.get_file_mutation_version(&root.join("batch-earlier.txt")), 1);
        let output = with_batch(
            &batch_tool,
            json!({"path":"batch-current.txt","edits":[{"op":"create","diff":"last\n"}]}),
            "deferred",
            true,
        )
        .await;
        assert!(!output.is_error);
        assert_eq!(deferred_batch.late.lock().unwrap().len(), 1);
        assert!(deferred_batch.events().iter().any(|event| event == "deferred-lookup:batch-earlier.txt:false"));
        assert!(deferred_batch.events().iter().any(|event| event == "deferred-lookup:batch-current.txt:true"));
        assert_eq!(deferred_batch.diagnostics_session.get_file_mutation_version(&root.join("batch-current.txt")), 1);

        // Actual EditTool inline transform wiring, with service installation
        // before constructor. The second identical inline result is reduced.
        let mut inline_host = Host::lsp();
        inline_host.diagnostics_available = true;
        inline_host.options = late_host.options;
        let inline_tool = tool(root, EditMode::Replace, &inline_host);
        std::fs::write(root.join("inline-tool.txt"), "a\n").unwrap();
        let output = run(&inline_tool, json!({"path":"inline-tool.txt","old_string":"a","new_string":"b"})).await;
        assert!(
            !output.is_error
                && !output.details.as_ref().unwrap()["diagnostics"]["messages"].as_array().unwrap().is_empty()
        );
        let output = run(&inline_tool, json!({"path":"inline-tool.txt","old_string":"b","new_string":"c"})).await;
        assert!(!output.is_error);
        assert!(output.details.as_ref().unwrap()["diagnostics"]["messages"].as_array().unwrap().is_empty());
        assert_eq!(output.details.as_ref().unwrap()["diagnostics"]["summary"], "no issues");

        // Fixed WRITE inline dedup is independent of the late queue supplier.
        let mut noqueue = Host::lsp();
        noqueue.inline_diagnostics_available = true;
        noqueue.options =
            EditLspOptions { enable_format: false, enable_diagnostics: true, deduplicate_diagnostics: true };
        let noqueue_write = WriteTool::new(ToolContext::new(root).with_edit_host(Arc::new(noqueue.clone())));
        let first =
            noqueue_write.execute_os_values("write-noqueue.txt", "first\n", CancellationToken::new()).await.unwrap();
        assert_eq!(first.details.as_ref().unwrap()["diagnostics"]["messages"], Host::diagnostics()["messages"]);
        let second =
            noqueue_write.execute_os_values("write-noqueue.txt", "second\n", CancellationToken::new()).await.unwrap();
        assert!(second.details.as_ref().unwrap()["diagnostics"]["messages"].as_array().unwrap().is_empty());
        assert_eq!(second.details.as_ref().unwrap()["diagnostics"]["summary"], "no issues");
        assert_eq!(*noqueue.diagnostic_consumers.lock().unwrap(), vec![(false, true), (false, true)]);
        assert!(noqueue.late.lock().unwrap().is_empty());
        assert_eq!(noqueue.diagnostics_session.pending_count(), 0);
        assert_eq!(std::fs::read_to_string(root.join("write-noqueue.txt")).unwrap(), "second\n");

        // Same shared Session batch: EDIT commits first, WRITE finalizes all
        // affected paths through its unrestricted fixed deferred callback.
        let mut shared = Host::lsp();
        shared.options =
            EditLspOptions { enable_format: false, enable_diagnostics: true, deduplicate_diagnostics: true };
        shared.diagnostics_available = true;
        shared.defer_diagnostics = true;
        let edit = tool(root, EditMode::Patch, &shared);
        let write = WriteTool::new(ToolContext::new(root).with_edit_host(Arc::new(shared.clone())));
        let first = with_batch(
            &edit,
            json!({"path":"shared-first.txt","edits":[{"op":"create","diff":"first\n"}]}),
            "mixed",
            false,
        )
        .await;
        assert!(!first.is_error);
        let final_write = write
            .execute_os_values_with_request(
                "shared-last.txt",
                "last\n",
                Some(EditLspBatch { id: "mixed".into(), flush: true }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(!final_write.is_error);
        assert_eq!(std::fs::read_to_string(root.join("shared-first.txt")).unwrap(), "first\n");
        assert_eq!(std::fs::read_to_string(root.join("shared-last.txt")).unwrap(), "last\n");
        assert!(shared.events().iter().any(|event| event == "deferred-lookup:shared-first.txt:true"));
        assert_eq!(shared.late.lock().unwrap().len(), 2);
        let late = shared.late.lock().unwrap()[0].clone();
        assert!(late.on_deferred_diagnostics(Host::diagnostics()).unwrap());
        write.execute_os_values("shared-first.txt", "newer\n", CancellationToken::new()).await.unwrap();
        assert!(shared.diagnostics_session.drain_lazy().survivors().is_empty());

        let captured_options = Arc::new(Mutex::new(EditLspOptions {
            enable_format: true,
            enable_diagnostics: false,
            deduplicate_diagnostics: false,
        }));
        let mut capture_host = Host::lsp();
        capture_host.options_override = Some(captured_options.clone());
        let captured_write = WriteTool::new(ToolContext::new(root).with_edit_host(Arc::new(capture_host)));
        *captured_options.lock().unwrap() = EditLspOptions::default();
        captured_write.execute_os_values("shared-capture.txt", "capture", CancellationToken::new()).await.unwrap();
        // b91 write.ts #lspWritethrough resolves settings for each write.
        assert_eq!(std::fs::read_to_string(root.join("shared-capture.txt")).unwrap(), "capture");
        *captured_options.lock().unwrap() = EditLspOptions { enable_format: true, ..EditLspOptions::default() };
        captured_write.execute_os_values("shared-capture.txt", "capture-again", CancellationToken::new()).await.unwrap();
        assert_eq!(std::fs::read_to_string(root.join("shared-capture.txt")).unwrap(), "formatted:capture-again");

        let mut formatted = Host::lsp();
        formatted.options = host.options;
        formatted.diagnostics_available = true;
        let write = WriteTool::new(
            ToolContext::new(root).with_edit(EditMode::Hashline, true).with_edit_host(Arc::new(formatted.clone())),
        );
        let output =
            write.execute_os_values("shared-inline.txt", "requested\n", CancellationToken::new()).await.unwrap();
        assert_eq!(std::fs::read_to_string(root.join("shared-inline.txt")).unwrap(), "formatted:requested\n");
        assert!(text(&output).contains(&ara_edit::store::file_hash("requested\n"))); // This legacy fake returns final_content=None; rich suppliers have separate coverage.
        assert_eq!(output.details.as_ref().unwrap()["diagnostics"]["errored"], true);
        assert_eq!(
            output.details.as_ref().unwrap()["meta"]["diagnostics"]["messages"],
            Host::diagnostics()["messages"]
        );
        exercise_permission_fallback_contracts(root).await;
    }

    // Controlled syscall failures, with successful handler effects on actual
    // temporary files. Virtual link/metadata rows do not prove OS symlinks or
    // a deployed privilege broker. This extends the existing coarse entrance.
    #[derive(Default)]
    struct FaultIo {
        faults: Mutex<BTreeMap<(&'static str, PathBuf), std::collections::VecDeque<std::io::ErrorKind>>>,
        events: Mutex<Vec<String>>,
        hidden: Mutex<Vec<PathBuf>>,
        links: Mutex<BTreeMap<PathBuf, PathBuf>>,
    }

    impl FaultIo {
        fn key(path: &Path) -> PathBuf {
            #[cfg(windows)]
            {
                let value = path.to_string_lossy();
                if let Some(tail) = value.strip_prefix(r"\\?\UNC\") {
                    return PathBuf::from(format!(r"\\{tail}"));
                }
                if let Some(tail) = value.strip_prefix(r"\\?\") {
                    return PathBuf::from(tail);
                }
            }
            path.to_owned()
        }
        fn fail(&self, op: &'static str, path: &Path, kinds: &[std::io::ErrorKind]) {
            self.faults.lock().unwrap().insert((op, Self::key(path)), kinds.iter().copied().collect());
        }
        fn enter(&self, op: &'static str, path: &Path) -> std::io::Result<()> {
            self.events.lock().unwrap().push(format!("{op}:{}", path.display()));
            if let Some(kind) =
                self.faults.lock().unwrap().get_mut(&(op, Self::key(path))).and_then(|faults| faults.pop_front())
            {
                return Err(std::io::Error::new(kind, format!("controlled {op} failure")));
            }
            Ok(())
        }
        fn metadata_visible(&self, path: &Path) -> std::io::Result<()> {
            if self.hidden.lock().unwrap().iter().any(|hidden| Self::key(path).starts_with(Self::key(hidden))) {
                Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "controlled metadata denial"))
            } else {
                Ok(())
            }
        }
        fn count(&self, op: &str) -> usize {
            self.events.lock().unwrap().iter().filter(|event| event.starts_with(&format!("{op}:"))).count()
        }
    }

    impl EditMutationFileSystem for FaultIo {
        fn write(&self, path: &Path, content: &str) -> std::io::Result<()> {
            self.enter("write", path)?;
            std::fs::write(path, content.as_bytes())
        }
        fn unlink(&self, path: &Path) -> std::io::Result<()> {
            self.enter("unlink", path)?;
            std::fs::remove_file(path)
        }
        fn mkdir(&self, path: &Path) -> std::io::Result<()> {
            self.enter("mkdir", path)?;
            std::fs::create_dir_all(path)
        }
        fn realpath(&self, path: &Path) -> std::io::Result<PathBuf> {
            self.enter("realpath", path)?;
            self.metadata_visible(path)?;
            let target = self.links.lock().unwrap().iter().find_map(|(link, target)| {
                Self::key(path)
                    .strip_prefix(Self::key(link))
                    .ok()
                    .map(|suffix| if suffix.as_os_str().is_empty() { target.clone() } else { target.join(suffix) })
            });
            std::fs::canonicalize(target.as_deref().unwrap_or(path))
        }
        fn lstat(&self, path: &Path) -> std::io::Result<EditMutationFileType> {
            self.enter("lstat", path)?;
            self.metadata_visible(path)?;
            if self.links.lock().unwrap().contains_key(&Self::key(path)) {
                return Ok(EditMutationFileType { is_symlink: true, is_directory: false, is_file: false });
            }
            OsEditMutationFileSystem.lstat(path)
        }
    }

    struct Handler {
        name: &'static str,
        accept: bool,
        fail: bool,
        events: Arc<Mutex<Vec<String>>>,
        dispose: Mutex<Option<EditFileFallbackRegistration>>,
    }
    impl Handler {
        fn new(name: &'static str, accept: bool, fail: bool, events: &Arc<Mutex<Vec<String>>>) -> Arc<Self> {
            Arc::new(Self { name, accept, fail, events: events.clone(), dispose: Mutex::new(None) })
        }
    }
    #[async_trait::async_trait]
    impl EditFileWriteFallback for Handler {
        async fn write(&self, request: &EditFileWriteFallbackRequest) -> std::io::Result<bool> {
            self.events.lock().unwrap().push(format!(
                "write:{}:{:?}:{}:{}:{:?}",
                self.name,
                request.session_id,
                request.dst.display(),
                request.content,
                request.cause.kind()
            ));
            let removed = self.dispose.lock().unwrap().take();
            if let Some(removed) = removed {
                removed.dispose();
            }
            if self.fail {
                return Err(std::io::Error::other("controlled handler threw"));
            }
            if self.accept {
                std::fs::create_dir_all(request.dst.parent().unwrap())?;
                std::fs::write(&request.dst, request.content.as_bytes())?;
            }
            Ok(self.accept)
        }
    }
    #[async_trait::async_trait]
    impl EditFileDeleteFallback for Handler {
        async fn delete(&self, request: &EditFileDeleteFallbackRequest) -> std::io::Result<bool> {
            self.events.lock().unwrap().push(format!(
                "delete:{}:{:?}:{}:{}:{:?}",
                self.name,
                request.session_id,
                request.dst.display(),
                request.confirmed_file,
                request.cause.kind()
            ));
            if self.fail {
                return Err(std::io::Error::other("controlled delete handler threw"));
            }
            if self.accept {
                std::fs::remove_file(&request.dst)?;
            }
            Ok(self.accept)
        }
    }

    async fn exercise_permission_fallback_contracts(root: &Path) {
        use std::io::ErrorKind::{InvalidInput, NotFound, PermissionDenied};
        let warnings = Arc::new(Mutex::new(Vec::new()));
        let observed = warnings.clone();
        let warning: EditFallbackWarningObserver = Arc::new(move |value| observed.lock().unwrap().push(value));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let io = Arc::new(FaultIo::default());
        let registry = EditFileMutationFallbackRegistry::new(io.clone(), warning.clone());
        let inert = root.join("inert.txt");
        io.fail("write", &inert, &[PermissionDenied]);
        assert_eq!(registry.write(&inert, "untouched", None).await.unwrap_err().original().kind(), PermissionDenied);
        assert_eq!(io.events.lock().unwrap().len(), 1, "unregistered seam performs no extra probes");
        assert!(!is_permission_denied(&std::io::Error::new(NotFound, "ENOENT /EACCES/file")));
        assert!(is_permission_denied(&std::io::Error::other("EACCES: plain transport error")));
        // Original five classifier rows, using native OS codes at the typed
        // Rust boundary rather than treating POSIX codes as Win32 errors.
        for kind in [PermissionDenied, std::io::ErrorKind::ReadOnlyFilesystem] {
            assert!(is_permission_denied(&std::io::Error::new(kind, "native denial")));
        }
        for kind in [NotFound, std::io::ErrorKind::IsADirectory, std::io::ErrorKind::StorageFull] {
            assert!(!is_permission_denied(&std::io::Error::new(kind, "unrelated errno")));
        }
        for message in ["something else went wrong", "not an error", "", "XEACCESsuffix"] {
            assert!(!is_permission_denied(&std::io::Error::other(message)));
        }
        for message in ["write failed: EACCES permission denied", "EPERM: denied", "EROFS: read-only"] {
            assert!(is_permission_denied(&std::io::Error::other(message)));
        }
        #[cfg(unix)]
        for code in [libc::EPERM, libc::EACCES, libc::EROFS] {
            assert!(is_permission_denied(&std::io::Error::from_raw_os_error(code)));
        }
        #[cfg(windows)]
        for code in [5, 19] {
            assert!(is_permission_denied(&std::io::Error::from_raw_os_error(code)));
        }

        let throwing = Handler::new("throw", false, true, &calls);
        let decline = Handler::new("decline", false, false, &calls);
        let accepted = Handler::new("accept", true, false, &calls);
        let _throw_registration = registry.add_write(throwing);
        let _decline_registration = registry.add_write(decline.clone());
        let accept_registration = registry.add_write(accepted.clone());
        let after_registration = registry.add_write(Handler::new("after-success", true, false, &calls));
        *decline.dispose.lock().unwrap() = Some(accept_registration);
        let ordinary = root.join("brokered.txt");
        io.fail("write", &ordinary, &[PermissionDenied]);
        registry.write(&ordinary, "exact: 汉字\n", Some("issuing-child-session")).await.unwrap();
        assert_eq!(std::fs::read(&ordinary).unwrap(), "exact: 汉字\n".as_bytes());
        let snapshot = calls.lock().unwrap().clone();
        assert!(snapshot[0].starts_with("write:throw:Some(\"issuing-child-session\")"));
        assert!(snapshot[1].starts_with("write:decline:"));
        assert!(snapshot[2].starts_with("write:accept:"), "disposing a live entry cannot skip its existing snapshot");
        assert_eq!(snapshot.len(), 3, "registration order stops at the first true handler");
        after_registration.dispose();
        assert_eq!(warnings.lock().unwrap().len(), 1);
        assert_eq!(warnings.lock().unwrap()[0].operation, "write");
        assert!(warnings.lock().unwrap()[0].error.contains("handler threw"));
        let removed = root.join("disposed.txt");
        io.fail("write", &removed, &[PermissionDenied]);
        assert_eq!(
            registry.write(&removed, "no accepted handler", None).await.unwrap_err().original().kind(),
            PermissionDenied
        );
        assert!(!removed.exists());
        let _accepted_again = registry.add_write(accepted);
        let unscoped = root.join("unscoped-write.txt");
        io.fail("write", &unscoped, &[PermissionDenied]);
        registry.write(&unscoped, "outside any tool call", None).await.unwrap();
        assert!(calls.lock().unwrap().last().unwrap().starts_with("write:accept:None:"));
        assert_eq!(std::fs::read_to_string(&unscoped).unwrap(), "outside any tool call");
        let shared_write_target = root.join("shared-denied-write.txt");
        io.fail("write", &shared_write_target, &[PermissionDenied]);
        let shared_write = WriteTool::new(ToolContext::new(root).with_edit_host(Arc::new(Host {
            file_fallbacks: Some(registry.clone()),
            mutation_session_id: Some("shared-write-session".into()),
            ..Default::default()
        })));
        let output = shared_write
            .execute_os_values("shared-denied-write.txt", "registered-handler", CancellationToken::new())
            .await
            .unwrap();
        assert!(!output.is_error);
        assert_eq!(std::fs::read_to_string(&shared_write_target).unwrap(), "registered-handler");
        assert!(
            calls.lock().unwrap().iter().any(|call| call.starts_with("write:accept:Some(\"shared-write-session\")"))
        );

        let before = calls.lock().unwrap().len();
        let race = root.join("race/once.txt");
        io.fail("write", &race, &[NotFound]);
        let writes = io.count("write");
        registry.write(&race, "repaired parent", None).await.unwrap();
        assert_eq!(io.count("write") - writes, 2);
        assert_eq!(std::fs::read_to_string(&race).unwrap(), "repaired parent");
        assert_eq!(calls.lock().unwrap().len(), before);
        // B-d2f92b2757: original persistent ENOENT, fresh creatable parent,
        // exactly one extra primitive attempt and observable directory repair.
        let repeated = root.join("persistent-race/fresh/path.txt");
        assert!(!repeated.parent().unwrap().exists());
        io.fail("write", &repeated, &[NotFound, NotFound]);
        let probes = io.count("mkdir");
        let repeated_writes = io.count("write");
        assert_eq!(registry.write(&repeated, "never committed", None).await.unwrap_err().original().kind(), NotFound);
        assert_eq!(io.count("write") - repeated_writes, 2);
        assert!(repeated.parent().unwrap().is_dir());
        assert_eq!(io.count("mkdir") - probes, 1);
        assert_eq!(calls.lock().unwrap().len(), before);
        let genuine = root.join("genuine.txt");
        io.fail("write", &genuine, &[InvalidInput]);
        let probes = io.count("realpath");
        assert_eq!(registry.write(&genuine, "bad path", None).await.unwrap_err().original().kind(), InvalidInput);
        assert_eq!(io.count("realpath"), probes);

        // Actual EditTool create/update/move/delete paths use the registry.
        // A denied parent mkdir is skipped only when a write handler exists;
        // the subsequent masked NotFound is classified using a second mkdir.
        let missing_parent = root.join("denied-parent");
        io.fail("mkdir", &missing_parent, &[PermissionDenied, PermissionDenied]);
        let mut host = Host::default();
        host.file_fallbacks = Some(registry.clone());
        host.mutation_session_id = Some("actual-edit-session".into());
        let edit = tool(root, EditMode::Patch, &host);
        let output = run(
            &edit,
            json!({"path":"denied-parent/new.txt","edits":[{"op":"create","diff":"broker creates parent\n"}]}),
        )
        .await;
        assert!(!output.is_error, "{}", text(&output));
        assert_eq!(std::fs::read_to_string(missing_parent.join("new.txt")).unwrap(), "broker creates parent\n");
        assert!(
            calls
                .lock()
                .unwrap()
                .iter()
                .any(|call| call.contains("Some(\"actual-edit-session\")") && call.ends_with("PermissionDenied"))
        );
        io.fail("write", &missing_parent.join("new.txt"), &[PermissionDenied]);
        let output = run(&edit, json!({"path":"denied-parent/new.txt","edits":[{"op":"update","diff":"@@\n-broker creates parent\n+updated"}]})).await;
        assert!(!output.is_error, "{}", text(&output));
        assert_eq!(std::fs::read_to_string(missing_parent.join("new.txt")).unwrap(), "updated\n");
        let mut lsp_host = Host::lsp();
        lsp_host.file_fallbacks = Some(registry.clone());
        lsp_host.mutation_session_id = Some("lsp-edit-session".into());
        let lsp_path = root.join("lsp-fallback.txt");
        io.fail("write", &lsp_path, &[PermissionDenied]);
        let output = run(
            &tool(root, EditMode::Patch, &lsp_host),
            json!({"path":"lsp-fallback.txt","edits":[{"op":"create","diff":"lsp shares byte seam\n"}]}),
        )
        .await;
        assert!(!output.is_error, "{}", text(&output));
        assert_eq!(std::fs::read_to_string(&lsp_path).unwrap(), "lsp shares byte seam\n");
        assert!(calls.lock().unwrap().iter().any(|call| call.contains("Some(\"lsp-edit-session\")")));

        // Delete is a separate registry. A registered writer cannot truncate
        // or otherwise claim a denied unlink, and NotFound is never hidden.
        let victim = root.join("victim.txt");
        std::fs::write(&victim, "keep bytes").unwrap();
        io.fail("unlink", &victim, &[PermissionDenied]);
        let before = calls.lock().unwrap().len();
        assert_eq!(
            registry.delete(&victim, Some("delete-session")).await.unwrap_err().original().kind(),
            PermissionDenied
        );
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep bytes");
        assert_eq!(calls.lock().unwrap().len(), before);
        assert_eq!(registry.delete(&root.join("write-only-missing.txt"), None).await.unwrap_err().original().kind(), NotFound);
        assert_eq!(calls.lock().unwrap().len(), before, "missing deletes never consult the registered writer");
        let delete_throw = Handler::new("delete-throw", false, true, &calls);
        let delete_accept = Handler::new("delete-accept", true, false, &calls);
        let _delete_throw = registry.add_delete(delete_throw);
        let _delete_accept = registry.add_delete(delete_accept);
        io.fail("unlink", &victim, &[PermissionDenied]);
        registry.delete(&victim, Some("delete-session")).await.unwrap();
        assert!(!victim.exists());
        assert!(calls.lock().unwrap().last().unwrap().contains(":true:PermissionDenied"));
        let before = calls.lock().unwrap().len();
        assert_eq!(registry.delete(&victim, None).await.unwrap_err().original().kind(), NotFound);
        assert_eq!(calls.lock().unwrap().len(), before);
        let directory = root.join("directory-must-stay");
        std::fs::create_dir(&directory).unwrap();
        io.fail("unlink", &directory, &[PermissionDenied]);
        assert_eq!(registry.delete(&directory, None).await.unwrap_err().original().kind(), PermissionDenied);
        assert!(directory.is_dir());
        assert_eq!(calls.lock().unwrap().len(), before);

        let hidden_file = root.join("hidden-metadata.txt");
        std::fs::write(&hidden_file, "delete without metadata").unwrap();
        io.hidden.lock().unwrap().push(hidden_file.clone());
        io.fail("unlink", &hidden_file, &[PermissionDenied]);
        registry.delete(&hidden_file, None).await.unwrap();
        assert!(!hidden_file.exists());
        assert!(calls.lock().unwrap().last().unwrap().contains(":false:PermissionDenied"));
        let hidden_ancestor = root.join("hidden-ancestor");
        std::fs::create_dir_all(&hidden_ancestor).unwrap();
        let hidden_target = hidden_ancestor.join("file.txt");
        std::fs::write(&hidden_target, "cannot prove target").unwrap();
        io.hidden.lock().unwrap().push(hidden_ancestor);
        io.fail("write", &hidden_target, &[PermissionDenied]);
        let before = calls.lock().unwrap().len();
        assert_eq!(
            registry.write(&hidden_target, "must refuse", None).await.unwrap_err().original().kind(),
            PermissionDenied
        );
        assert_eq!(std::fs::read_to_string(&hidden_target).unwrap(), "cannot prove target");
        assert_eq!(calls.lock().unwrap().len(), before);

        // Controlled final-link model: write follows it, delete preserves it.
        // The two actual backing files let us inspect the intended effects;
        // native platform symlink/junction acceptance remains Root-owned.
        let link = root.join("link-placeholder.txt");
        let target = root.join("link-target.txt");
        std::fs::write(&link, "link itself").unwrap();
        std::fs::write(&target, "original target").unwrap();
        io.links.lock().unwrap().insert(link.clone(), target.clone());
        io.fail("write", &link, &[PermissionDenied]);
        registry.write(&link, "followed target", None).await.unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "followed target");
        assert_eq!(std::fs::read_to_string(&link).unwrap(), "link itself");
        io.fail("unlink", &link, &[PermissionDenied]);
        registry.delete(&link, None).await.unwrap();
        assert!(!link.exists());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "followed target");
        assert!(calls.lock().unwrap().last().unwrap().contains(":false:PermissionDenied"));

        // EDIT move destination and source unlink both use their proper seam.
        let source = missing_parent.join("new.txt");
        let destination = root.join("moved-fallback.txt");
        io.fail("write", &destination, &[PermissionDenied]);
        io.fail("unlink", &source, &[PermissionDenied]);
        let output = run(&edit, json!({"path":"denied-parent/new.txt","edits":[{"op":"update","rename":"moved-fallback.txt","diff":"@@\n-updated\n+moved"}]})).await;
        assert!(!output.is_error, "{}", text(&output));
        assert!(!source.exists());
        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "moved\n");
        io.fail("unlink", &destination, &[PermissionDenied]);
        let output = run(&edit, json!({"path":"moved-fallback.txt","edits":[{"op":"delete"}]})).await;
        assert!(!output.is_error, "{}", text(&output));
        assert!(!destination.exists());

        let rejected_io = Arc::new(FaultIo::default());
        let rejected = EditFileMutationFallbackRegistry::new(rejected_io.clone(), warning);
        let _reject = rejected.add_write(Handler::new("reject", false, false, &calls));
        let masked = root.join("rejected-parent/file.txt");
        rejected_io.fail("write", &masked, &[NotFound]);
        rejected_io.fail("mkdir", masked.parent().unwrap(), &[PermissionDenied]);
        let failure = rejected.write(&masked, "rejected bytes", None).await.unwrap_err();
        assert_eq!(failure.original().kind(), NotFound);
        assert_eq!(failure.boundary_cause().unwrap().kind(), PermissionDenied);
        assert!(!masked.exists());
        struct CaptureWrite {
            cause: Arc<Mutex<Option<Arc<std::io::Error>>>>,
            throw: bool,
        }
        #[async_trait::async_trait]
        impl EditFileWriteFallback for CaptureWrite {
            async fn write(&self, request: &EditFileWriteFallbackRequest) -> std::io::Result<bool> {
                *self.cause.lock().unwrap() = Some(request.cause.clone());
                if self.throw { Err(std::io::Error::other("write handler failed")) } else { Ok(false) }
            }
        }
        for throw in [false, true] {
            let original_registry = EditFileMutationFallbackRegistry::new(Arc::new(OsEditMutationFileSystem), Arc::new(|_| {}));
            let cause = Arc::new(Mutex::new(None));
            let _registration = original_registry.add_write(Arc::new(CaptureWrite { cause: cause.clone(), throw }));
            let target = root.join(format!("original-write-error-{throw}.txt"));
            let failure = original_registry.write_using(&target, "never persisted", None, || {
                Err(std::io::Error::new(PermissionDenied, "original write denial"))
            }).await.unwrap_err();
            let captured = cause.lock().unwrap().clone().unwrap();
            assert!(std::ptr::eq(captured.as_ref(), failure.original()));
            assert_eq!(failure.original().to_string(), "original write denial");
            assert!(!target.exists());
        }
        // Fixed B-bf15c05be4 file-handle primitive, through the same registry.
        // The callback is supplied per call and never replaced or retried by
        // the default IO primitive. Decline/throw preserves the original object.
        struct CaptureDelete {
            cause: Arc<Mutex<Option<Arc<std::io::Error>>>>,
            throw: bool,
        }
        #[async_trait::async_trait]
        impl EditFileDeleteFallback for CaptureDelete {
            async fn delete(&self, request: &EditFileDeleteFallbackRequest) -> std::io::Result<bool> {
                *self.cause.lock().unwrap() = Some(request.cause.clone());
                assert_eq!(request.session_id.as_deref(), Some("handle-session"));
                assert!(request.confirmed_file);
                if self.throw { Err(std::io::Error::other("handler failed")) } else { Ok(false) }
            }
        }
        for throw in [false, true] {
            let primitive_io = Arc::new(FaultIo::default());
            let handle_registry = EditFileMutationFallbackRegistry::new(primitive_io.clone(), Arc::new(|_| {}));
            let handle_target = root.join(format!("handle-delete-{throw}.txt"));
            std::fs::write(&handle_target, "keep handle bytes").unwrap();
            let cause = Arc::new(Mutex::new(None));
            let _registration = handle_registry.add_delete(Arc::new(CaptureDelete { cause: cause.clone(), throw }));
            let attempts = std::sync::atomic::AtomicUsize::new(0);
            let failure = handle_registry.delete_using(&handle_target, Some("handle-session"), || {
                attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(std::io::Error::new(PermissionDenied, "original handle denial"))
            }).await.unwrap_err();
            assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
            assert_eq!(primitive_io.count("unlink"), 0);
            let captured = cause.lock().unwrap().clone().unwrap();
            assert!(std::ptr::eq(captured.as_ref(), failure.original()));
            assert_eq!(failure.original().to_string(), "original handle denial");
            assert_eq!(std::fs::read_to_string(&handle_target).unwrap(), "keep handle bytes");
        }
        let no_handlers = EditFileMutationFallbackRegistry::new(Arc::new(FaultIo::default()), Arc::new(|_| {}));
        assert!(!no_handlers.has_write_fallback() && !no_handlers.has_delete_fallback());
        #[cfg(unix)]
        exercise_real_permission_fallback_contracts().await;
    }

    // Original eight write and eight delete real-kernel rows stay under the
    // existing LSP/fallback coarse family. A root or Windows receipt cannot
    // count these as executed; actual non-root Unix output is required.
    #[cfg(unix)]
    async fn exercise_real_permission_fallback_contracts() {
        use std::io::ErrorKind::{NotADirectory, NotFound, PermissionDenied};
        use std::os::unix::fs::{PermissionsExt, symlink};
        if unsafe { libc::geteuid() } == 0 {
            eprintln!("WRITE_KERNEL16_UNEXECUTED: uid=0 cannot witness mode-bit denials");
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        struct Restore(Vec<PathBuf>);
        impl Drop for Restore {
            fn drop(&mut self) {
                for path in self.0.iter().rev() {
                    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
                }
            }
        }
        let mut restore = Restore(Vec::new());
        let mut lock = |name: &str, file: Option<&str>| {
            let path = root.join(name);
            std::fs::create_dir(&path).unwrap();
            if let Some(file) = file { std::fs::write(path.join(file), "payload").unwrap(); }
            restore.0.push(path.clone());
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o500)).unwrap();
            path
        };
        type WriteReceipt = (PathBuf, String, Option<String>, std::io::ErrorKind, Option<i32>);
        type DeleteReceipt = (PathBuf, bool, Option<String>, std::io::ErrorKind);
        struct KernelHandler {
            accept: bool,
            writes: Mutex<Vec<WriteReceipt>>,
            deletes: Mutex<Vec<DeleteReceipt>>,
        }
        impl KernelHandler {
            fn new(accept: bool) -> Arc<Self> {
                Arc::new(Self { accept, writes: Mutex::new(Vec::new()), deletes: Mutex::new(Vec::new()) })
            }
        }
        #[async_trait::async_trait]
        impl EditFileWriteFallback for KernelHandler {
            async fn write(&self, request: &EditFileWriteFallbackRequest) -> std::io::Result<bool> {
                self.writes.lock().unwrap().push((request.dst.clone(), request.content.clone(), request.session_id.clone(), request.cause.kind(), request.cause.raw_os_error()));
                Ok(self.accept)
            }
        }
        #[async_trait::async_trait]
        impl EditFileDeleteFallback for KernelHandler {
            async fn delete(&self, request: &EditFileDeleteFallbackRequest) -> std::io::Result<bool> {
                self.deletes.lock().unwrap().push((request.dst.clone(), request.confirmed_file, request.session_id.clone(), request.cause.kind()));
                Ok(self.accept)
            }
        }
        let registry = EditFileMutationFallbackRegistry::new(Arc::new(OsEditMutationFileSystem), Arc::new(|_| {}));
        let accepted = KernelHandler::new(true);
        let write_registration = registry.add_write(accepted.clone());
        let mut observed = 0;
        // W1: real denied create names the canonical destination and bytes.
        let locked = lock("write-denied", None);
        let target = locked.join("new.txt");
        registry.write(&target, "payload", None).await.unwrap();
        assert_eq!(accepted.writes.lock().unwrap().last().unwrap(), &(target.clone(), "payload".into(), None, PermissionDenied, Some(libc::EACCES)));
        assert!(!target.exists());
        observed += 1;
        assert_eq!(accepted.writes.lock().unwrap().len(), 1);
        // W2: a real ENOENT is unmasked by the denied parent mkdir.
        let masked = locked.join("sub/new.txt");
        registry.write(&masked, "payload", None).await.unwrap();
        assert_eq!(accepted.writes.lock().unwrap().last().unwrap(), &(masked, "payload".into(), None, PermissionDenied, Some(libc::EACCES)));
        assert_eq!(accepted.writes.lock().unwrap().len(), 2);
        observed += 1;
        // W3: declining the recovered boundary preserves original ENOENT.
        write_registration.dispose();
        let declined = KernelHandler::new(false);
        let decline_registration = registry.add_write(declined.clone());
        let failure = registry.write(&locked.join("declined/new.txt"), "payload", None).await.unwrap_err();
        assert_eq!(failure.original().kind(), NotFound);
        assert_eq!(failure.boundary_cause().unwrap().kind(), PermissionDenied);
        assert_eq!(failure.original().raw_os_error(), Some(libc::ENOENT));
        assert_eq!(failure.boundary_cause().unwrap().raw_os_error(), Some(libc::EACCES));
        observed += 1;
        // W4: a file ancestor is not a permission boundary.
        let blocker = root.join("blocker");
        std::fs::write(&blocker, "not a directory").unwrap();
        let before = declined.writes.lock().unwrap().len();
        let failure = registry.write(&blocker.join("child.txt"), "payload", None).await.unwrap_err();
        assert!(matches!(failure.original().kind(), NotFound | NotADirectory));
        assert_eq!(declined.writes.lock().unwrap().len(), before);
        observed += 1;
        // W5/W6: final and ancestral symlinks both name the protected target.
        let secret_dir = lock("off-limits-write", Some("secret.txt"));
        let secret = secret_dir.join("secret.txt");
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o400)).unwrap();
        let link = root.join("innocent-link");
        symlink(&secret, &link).unwrap();
        assert_eq!(registry.write(&link, "pwned\n", None).await.unwrap_err().original().kind(), PermissionDenied);
        assert_eq!(declined.writes.lock().unwrap().last().unwrap().0, secret);
        assert_eq!(std::fs::read_to_string(&secret).unwrap(), "payload");
        observed += 1;
        assert_eq!(declined.writes.lock().unwrap().len(), before + 1);
        let link_dir = root.join("innocent-dir");
        symlink(&secret_dir, &link_dir).unwrap();
        assert_eq!(registry.write(&link_dir.join("secret.txt"), "pwned\n", None).await.unwrap_err().original().kind(), PermissionDenied);
        assert_eq!(declined.writes.lock().unwrap().last().unwrap().0, secret);
        assert_eq!(std::fs::read_to_string(&secret).unwrap(), "payload");
        observed += 1;
        assert_eq!(declined.writes.lock().unwrap().len(), before + 2);
        // W7: a dangling final symlink has no proven broker destination.
        let dangling_dir = lock("dangling-parent", None);
        std::fs::set_permissions(&dangling_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let dangling = dangling_dir.join("dangling");
        symlink(dangling_dir.join("nowhere"), &dangling).unwrap();
        std::fs::set_permissions(&dangling_dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let before = declined.writes.lock().unwrap().len();
        assert_eq!(registry.write(&dangling, "payload", None).await.unwrap_err().original().kind(), PermissionDenied);
        assert_eq!(declined.writes.lock().unwrap().len(), before);
        observed += 1;
        // W8: metadata hidden behind a real mode-zero ancestor fails closed.
        let opaque_write = lock("opaque-write", None);
        std::fs::set_permissions(&opaque_write, std::fs::Permissions::from_mode(0)).unwrap();
        assert_eq!(registry.write(&opaque_write.join("new.txt"), "payload", None).await.unwrap_err().original().kind(), PermissionDenied);
        assert_eq!(declined.writes.lock().unwrap().len(), before);
        observed += 1;
        decline_registration.dispose();
        let deleted = KernelHandler::new(true);
        let delete_registration = registry.add_delete(deleted.clone());
        // D1: actual denied unlink retains the issuing Session.
        let delete_dir = lock("delete-denied", Some("victim.txt"));
        let victim = delete_dir.join("victim.txt");
        registry.delete(&victim, Some("session-del")).await.unwrap();
        assert_eq!(deleted.deletes.lock().unwrap().last().unwrap(), &(victim.clone(), true, Some("session-del".into()), PermissionDenied));
        observed += 1;
        assert_eq!(deleted.deletes.lock().unwrap().len(), 1);
        // D2: a declining handler retains the real denial.
        delete_registration.dispose();
        let delete_declined = KernelHandler::new(false);
        let decline_delete_registration = registry.add_delete(delete_declined.clone());
        assert_eq!(registry.delete(&victim, None).await.unwrap_err().original().kind(), PermissionDenied);
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "payload");
        observed += 1;
        decline_delete_registration.dispose();
        let _delete_registration = registry.add_delete(deleted.clone());
        // D3: never broker a confirmed directory; a real file is confirmed.
        let confirmed_dir = root.join("confirmed-directory");
        std::fs::create_dir(&confirmed_dir).unwrap();
        let before = deleted.deletes.lock().unwrap().len();
        let directory_failure = registry.delete(&confirmed_dir, None).await.unwrap_err();
        assert!(matches!(directory_failure.original().kind(), PermissionDenied | std::io::ErrorKind::IsADirectory));
        assert!(confirmed_dir.is_dir());
        assert_eq!(deleted.deletes.lock().unwrap().len(), before);
        registry.delete(&victim, None).await.unwrap();
        assert!(deleted.deletes.lock().unwrap().last().unwrap().1);
        observed += 1;
        assert_eq!(deleted.deletes.lock().unwrap().len(), before + 1);
        // D4: actual lstat denial is reported as unresolved, not a file.
        let opaque_delete = lock("opaque-delete", Some("buried.txt"));
        std::fs::set_permissions(&opaque_delete, std::fs::Permissions::from_mode(0)).unwrap();
        let buried = opaque_delete.join("buried.txt");
        let before = deleted.deletes.lock().unwrap().len();
        registry.delete(&buried, None).await.unwrap();
        assert_eq!(deleted.deletes.lock().unwrap().last().unwrap().0, buried);
        assert!(!deleted.deletes.lock().unwrap().last().unwrap().1);
        observed += 1;
        assert_eq!(deleted.deletes.lock().unwrap().len(), before + 1);
        // D5: ENOTDIR never becomes a privileged delete request.
        let before = deleted.deletes.lock().unwrap().len();
        assert_eq!(registry.delete(&blocker.join("child.txt"), None).await.unwrap_err().original().kind(), NotADirectory);
        assert_eq!(deleted.deletes.lock().unwrap().len(), before);
        observed += 1;
        // D6: unlink follows an ancestral symlink, and names that victim.
        let outside_delete = lock("off-limits-delete", Some("keep.txt"));
        let delete_link_dir = root.join("delete-link-dir");
        symlink(&outside_delete, &delete_link_dir).unwrap();
        _delete_registration.dispose();
        let decline_ancestor = registry.add_delete(delete_declined.clone());
        let before = delete_declined.deletes.lock().unwrap().len();
        assert_eq!(registry.delete(&delete_link_dir.join("keep.txt"), None).await.unwrap_err().original().kind(), PermissionDenied);
        assert_eq!(delete_declined.deletes.lock().unwrap()[before].0, outside_delete.join("keep.txt"));
        assert!(delete_declined.deletes.lock().unwrap()[before].1);
        assert_eq!(std::fs::read_to_string(outside_delete.join("keep.txt")).unwrap(), "payload");
        observed += 1;
        assert_eq!(delete_declined.deletes.lock().unwrap().len(), before + 1);
        decline_ancestor.dispose();
        let _delete_registration = registry.add_delete(deleted.clone());
        // D7: final symlinks are brokered as links; their directory survives.
        let tree = root.join("link-target-tree");
        std::fs::create_dir(&tree).unwrap();
        std::fs::write(tree.join("keep.txt"), "keep me").unwrap();
        let final_link_parent = lock("final-link-parent", None);
        std::fs::set_permissions(&final_link_parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        let final_link = final_link_parent.join("dir-link");
        symlink(&tree, &final_link).unwrap();
        std::fs::set_permissions(&final_link_parent, std::fs::Permissions::from_mode(0o500)).unwrap();
        let before = deleted.deletes.lock().unwrap().len();
        registry.delete(&final_link, None).await.unwrap();
        assert_eq!(deleted.deletes.lock().unwrap().last().unwrap().0, final_link);
        assert!(!deleted.deletes.lock().unwrap().last().unwrap().1);
        assert_eq!(std::fs::read_to_string(tree.join("keep.txt")).unwrap(), "keep me");
        observed += 1;
        assert_eq!(deleted.deletes.lock().unwrap().len(), before + 1);
        // D8: the per-call handle primitive performs a real denied unlink once.
        let retained_handle = std::fs::File::open(&victim).unwrap();
        let before = deleted.deletes.lock().unwrap().len();
        let attempts = std::sync::atomic::AtomicUsize::new(0);
        registry.delete_using(&victim, Some("handle-session"), || {
            let _keep_handle = &retained_handle;
            attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            std::fs::remove_file(&victim)
        }).await.unwrap();
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(deleted.deletes.lock().unwrap().last().unwrap().0, victim);
        observed += 1;
        assert_eq!(deleted.deletes.lock().unwrap().len(), before + 1);
        assert_eq!(observed, 16);
        eprintln!("WRITE_KERNEL16_EXECUTED: uid={} observations={observed}", unsafe { libc::geteuid() });
    }
}

struct Tools {
    read: ReadTool,
    grep: GrepTool,
    edit: EditTool,
}

fn tools(root: &Path, mode: EditMode) -> Tools {
    let ctx = ToolContext::new(root).with_edit(mode, true);
    Tools { read: ReadTool { ctx: ctx.clone() }, grep: GrepTool::new(ctx.clone()), edit: EditTool::new(ctx) }
}

#[tokio::test]
async fn replace_mode_results_and_errors() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    std::fs::write(r.join("a.py"), GREET).unwrap();
    let t = tools(r, EditMode::Replace);
    // Replace mode keeps plain reads (no hashline anchors).
    assert_eq!(text(&run(&t.read, json!({"path": "a.py"})).await), GREET.trim_end());
    let out =
        run(&t.edit, json!({"path": "a.py", "old_string": "print(msg)", "new_string": "print(msg.upper())"})).await;
    assert!(!out.is_error);
    assert_eq!(
        text(&out),
        "[a.py]\n1:def greet(name):\n2:    msg = \"Hello, \" + name\n3:    print(msg.upper())\n4:greet(\"world\")"
    );
    let details = out.details.unwrap();
    assert_eq!(details["op"], "update");
    assert_eq!(details["firstChangedLine"], 3);
    assert!(details["diff"].as_str().unwrap().contains("-3|    print(msg)"), "{details}");
    assert_eq!(std::fs::read_to_string(r.join("a.py")).unwrap(), GREET.replace("print(msg)", "print(msg.upper())"));

    let missing = run(&t.edit, json!({"path": "a.py", "old_string": "nope nope", "new_string": "x"})).await;
    assert!(missing.is_error);
    assert!(text(&missing).starts_with("Could not find a close enough match in a.py."), "{}", text(&missing));
    let multi = run(&t.edit, json!({"path": "a.py", "old_string": "msg", "new_string": "m"})).await;
    assert!(multi.is_error && text(&multi).starts_with("Found 2 occurrences in a.py:"));
    assert!(text(&multi).ends_with("Add more context lines to disambiguate."));
    let all = run(&t.edit, json!({"path": "a.py", "old_string": "msg", "new_string": "m", "replace_all": true})).await;
    assert!(!all.is_error, "{}", text(&all));
    assert!(!std::fs::read_to_string(r.join("a.py")).unwrap().contains("msg"));
    let nofile = run(&t.edit, json!({"path": "zz.py", "old_string": "a", "new_string": "b"})).await;
    assert!(nofile.is_error);
    assert_eq!(text(&nofile), "File not found: zz.py");
    // Fuzzy whitespace match (edit.fuzzyMatch default true).
    std::fs::write(r.join("w.txt"), "alpha  beta\n").unwrap();
    let fuzzy = run(&t.edit, json!({"path": "w.txt", "old_string": "alpha beta", "new_string": "gamma"})).await;
    assert!(!fuzzy.is_error, "{}", text(&fuzzy));
    assert_eq!(std::fs::read_to_string(r.join("w.txt")).unwrap(), "gamma\n");
}

#[tokio::test]
async fn hashline_anchors_flow_from_read_and_grep_to_edit() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    std::fs::write(r.join("a.py"), GREET).unwrap();
    let t = tools(r, EditMode::Hashline);
    let first = tag(r, "a.py");
    assert_eq!(
        text(&run(&t.read, json!({"path": "a.py"})).await),
        format!(
            "[a.py#{first}]\n1:def greet(name):\n2:    msg = \"Hello, \" + name\n3:    print(msg)\n4:greet(\"world\")"
        )
    );
    assert_eq!(
        text(&run(&t.grep, json!({"pattern": "msg", "path": "a.py"})).await),
        format!(
            "[a.py#{first}]\n 1:def greet(name):\n*2:    msg = \"Hello, \" + name\n*3:    print(msg)\n 4:greet(\"world\")"
        )
    );
    let put = run(&t.edit, json!({"input": format!("[a.py#{first}]\nPUT 3.=3:\n+    print(msg + \"!\")\n")})).await;
    let second = tag(r, "a.py");
    assert_ne!(first, second);
    assert_eq!(
        text(&put),
        format!(
            "[a.py#{second}]\n1:def greet(name):\n2:    msg = \"Hello, \" + name\n3:    print(msg + \"!\")\n4:greet(\"world\")"
        )
    );
    // A stale tag is rejected with the current content and the new tag.
    let stale = run(&t.edit, json!({"input": format!("[a.py#{first}]\nPUT 3.=3:\n+    print(1)\n")})).await;
    assert!(stale.is_error);
    assert!(
        text(&stale).starts_with(&format!(
            "Edit rejected for a.py: file changed between read and edit.\nSection is bound to #{first}, but the current file hashes to #{second}."
        )),
        "{}",
        text(&stale)
    );
    assert!(std::fs::read_to_string(r.join("a.py")).unwrap().contains("print(msg + \"!\")"), "nothing written");

    // Grouped grep headers carry tags too.
    std::fs::create_dir_all(r.join("src")).unwrap();
    std::fs::write(r.join("src/b.py"), "msg = 1\n").unwrap();
    let t_b = tag(r, "src/b.py");
    let grouped = text(&run(&t.grep, json!({"pattern": "msg = 1"})).await);
    assert_eq!(grouped, format!("# src/\n## b.py#{t_b}\n*1:msg = 1"));
    // Insert after the last line using the grep tag.
    let ins = run(&t.edit, json!({"input": format!("[src/b.py#{t_b}]\nPUT >1:\n+msg = 2\n")})).await;
    assert!(!ins.is_error, "{}", text(&ins));
    assert_eq!(std::fs::read_to_string(r.join("src/b.py")).unwrap(), "msg = 1\nmsg = 2\n");
}

#[tokio::test]
async fn disjoint_read_context_line_is_a_valid_hashline_anchor() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let original = "function one() {\n  const x = 1;\n  return x;\n}\nfunction two() {\n  return 2;\n}\n";
    std::fs::write(root.join("blocks.ts"), original).unwrap();
    let t = tools(root, EditMode::Hashline);
    let read = text(&run(&t.read, json!({"path":"blocks.ts:1-1,5-5"})).await);
    assert!(read.contains("\n4:}\n5:function two() {"), "{read}");
    let initial_tag = tag(root, "blocks.ts");
    let visible = run(&t.edit, json!({"input": format!("[blocks.ts#{initial_tag}]\nPUT 4.=4:\n+}} // first\n")})).await;
    assert!(!visible.is_error, "{}", text(&visible));
    assert!(std::fs::read_to_string(root.join("blocks.ts")).unwrap().contains("} // first"));
}

#[tokio::test]
async fn hashline_blocks_moves_deletes_and_parse_warnings() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    std::fs::write(r.join("lib.rs"), "fn main() {\n    let x = 1;\n    println!(\"{x}\");\n}\n").unwrap();
    let t = tools(r, EditMode::Hashline);
    // `PUT 1*:` resolves the whole function through the tree-sitter block walk.
    let block = run(&t.edit, json!({"input": format!("[lib.rs#{}]\nPUT 1*:\n+fn main() {{\n+    println!(\"hi\");\n+}}\n", tag(r, "lib.rs"))})).await;
    assert!(text(&block).contains("PUT 1*: → resolved lines 1-4 (4 lines)"), "{}", text(&block));
    assert_eq!(std::fs::read_to_string(r.join("lib.rs")).unwrap(), "fn main() {\n    println!(\"hi\");\n}\n");
    // Breaking the syntax is applied, with the engine warning and the host note.
    let broken =
        run(&t.edit, json!({"input": format!("[lib.rs#{}]\nPUT 2.=2:\n+    println!(\"hi\";\n", tag(r, "lib.rs"))}))
            .await;
    assert!(!broken.is_error);
    let tb = text(&broken);
    assert!(tb.contains("Warnings:\nThis edit introduced a syntax error near line 2"), "{tb}");
    assert!(tb.ends_with("<<>>\nWarning: lib.rs no longer parses after this edit. The change was applied; re-read the edited region and fix the syntax, or revert if unintended."), "{tb}");

    // Move, then delete.
    std::fs::write(r.join("a.py"), GREET).unwrap();
    let mv = run(&t.edit, json!({"input": format!("[a.py#{}]\nMV lib/greet.py\n", tag(r, "a.py"))})).await;
    assert!(text(&mv).starts_with("[lib/greet.py#") && text(&mv).contains("Moved to lib/greet.py"), "{}", text(&mv));
    assert!(!r.join("a.py").exists() && std::fs::read_to_string(r.join("lib/greet.py")).unwrap() == GREET);
    let rem = run(&t.edit, json!({"input": format!("[lib/greet.py#{}]\nREM\n", tag(r, "lib/greet.py"))})).await;
    assert_eq!(text(&rem), "Deleted lib/greet.py");
    assert!(!r.join("lib/greet.py").exists());

    // Cut and paste through a named register across sections.
    std::fs::write(r.join("x.py"), "a = 1\nb = 2\n").unwrap();
    std::fs::write(r.join("y.py"), "c = 3\n").unwrap();
    let cut = format!("[x.py#{}]\nCUT 1.=1 @v\n[y.py#{}]\nPUT >1 @v\n", tag(r, "x.py"), tag(r, "y.py"));
    let out = run(&t.edit, json!({"input": cut})).await;
    assert!(!out.is_error, "{}", text(&out));
    assert_eq!(std::fs::read_to_string(r.join("x.py")).unwrap(), "b = 2\n");
    assert_eq!(std::fs::read_to_string(r.join("y.py")).unwrap(), "c = 3\na = 1\n");
    assert!(out.details.unwrap()["perFileResults"].as_array().is_some_and(|a| a.len() == 2));
}

#[tokio::test]
async fn patch_and_apply_patch_modes() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    std::fs::write(r.join("app.py"), "def greet():\n    print('Hi')\n").unwrap();
    let t = tools(r, EditMode::Patch);
    let upd = run(&t.edit, json!({"path": "app.py", "edits": [{"op": "update", "diff": "@@ def greet():\n def greet():\n-    print('Hi')\n+    print('Hello')\n"}]})).await;
    assert!(!upd.is_error, "{}", text(&upd));
    assert_eq!(std::fs::read_to_string(r.join("app.py")).unwrap(), "def greet():\n    print('Hello')\n");
    let create = run(&t.edit, json!({"path": "new/hello.txt", "edits": [{"op": "create", "diff": "Hello\n"}]})).await;
    assert!(!create.is_error, "{}", text(&create));
    assert_eq!(std::fs::read_to_string(r.join("new/hello.txt")).unwrap(), "Hello\n");

    let t = tools(r, EditMode::ApplyPatch);
    let patch = "*** Begin Patch\n*** Update File: app.py\n*** Move to: src/main.py\n@@ def greet():\n-    print('Hello')\n+    print('Hello, world!')\n*** Delete File: new/hello.txt\n*** End Patch\n";
    let out = run(&t.edit, json!({"input": patch})).await;
    assert!(!out.is_error, "{}", text(&out));
    assert!(!r.join("app.py").exists() && !r.join("new/hello.txt").exists());
    assert_eq!(std::fs::read_to_string(r.join("src/main.py")).unwrap(), "def greet():\n    print('Hello, world!')\n");
    let bad =
        run(&t.edit, json!({"input": "*** Begin Patch\n*** Update File: missing.py\n@@\n-a\n+b\n*** End Patch\n"}))
            .await;
    assert!(bad.is_error, "{}", text(&bad));
}

#[tokio::test]
async fn tool_surface_and_read_edge_cases() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    let ctx = ToolContext::new(r).with_edit(EditMode::Hashline, true).with_read_output_max_columns(0.0);
    let edit = EditTool::new(ctx.clone());
    let def = edit.definition();
    assert_eq!(def.name, "edit");
    assert_eq!(def.description, ara_edit::description(EditMode::Hashline));
    assert_eq!(def.parameters["required"], json!(["input"]));
    assert_eq!(edit.concurrency(&JsonObject::new()), Concurrency::Exclusive);
    let replace = EditTool::new(ToolContext::new(r).with_edit(EditMode::Replace, true));
    assert_eq!(replace.definition().parameters["required"], json!(["path", "old_string", "new_string"]));
    let names: Vec<String> = builtin_tools(ctx.clone()).iter().map(|t| t.definition().name).collect();
    assert_eq!(names, ["read", "write", "edit", "bash", "grep", "glob"]);
    // Hashline display needs the edit tool: disabled edit keeps plain reads.
    assert!(!ToolContext::new(r).with_edit(EditMode::Hashline, false).hashlines());
    assert!(ToolContext::new(r).hashlines(), "upstream default: edit tool exposed in hashline mode");

    let read = ReadTool { ctx: ctx.clone() };
    std::fs::write(r.join("a.py"), GREET).unwrap();
    let raw = text(&run(&read, json!({"path": "a.py:raw"})).await);
    assert_eq!(raw, GREET, "raw reads preserve the terminal newline and carry no anchors");
    let range = text(&run(&read, json!({"path": "a.py:2-3"})).await);
    assert_eq!(
        range,
        format!(
            "[a.py#{}]\n1:def greet(name):\n2:    msg = \"Hello, \" + name\n3:    print(msg)\n4:greet(\"world\")",
            tag(r, "a.py")
        )
    );
    let wide = format!("{}\n", "x".repeat(60 * 1024));
    std::fs::write(r.join("wide.txt"), &wide).unwrap();
    assert_eq!(
        text(&run(&read, json!({"path": "wide.txt:1-1"})).await),
        "[Line 1 is 60.0KB, exceeds 50.0KB limit. Hashline output requires full lines; cannot emit an editable numbered preview for a truncated line.]"
    );
    // A new file comes from `write`; hashline edits need an existing file.
    let write = WriteTool::new(ctx.clone());
    run(&write, json!({"path": "n.txt", "content": "one\n"})).await;
    let out = run(&edit, json!({"input": format!("[n.txt#{}]\nPUT 1.=1:\n+uno\n", tag(r, "n.txt"))})).await;
    assert!(!out.is_error, "{}", text(&out));
    assert_eq!(std::fs::read_to_string(r.join("n.txt")).unwrap(), "uno\n");
    let cancel = CancellationToken::new();
    cancel.cancel();
    let e = edit.execute("c", args(json!({"input": "x"})), cancel, Arc::new(|_| {})).await.unwrap_err();
    assert_eq!(e.0, "Edit was aborted");
}

#[tokio::test]
async fn upstream_file_reencoding_moves_and_partial_failure_report() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    let t = tools(r, EditMode::Hashline);
    // b91db66e replaces historical lossy reencoding with strict UTF-8 staging refusal.
    let latin1 = b"caf\xe9 au lait\nsecond\nthird\n".to_vec();
    std::fs::write(r.join("l1.txt"), &latin1).unwrap();
    let rt = tools(r, EditMode::Replace);
    let out = run(&rt.edit, json!({"path": "l1.txt", "old_string": "third", "new_string": "THIRD"})).await;
    assert!(out.is_error, "{}", text(&out));
    assert!(text(&out).contains("not valid UTF-8"), "{}", text(&out));
    assert_eq!(std::fs::read(r.join("l1.txt")).unwrap(), latin1);

    // Fixed OMP hashline MV replaces an existing destination.
    std::fs::write(r.join("a.txt"), "AAA\n").unwrap();
    std::fs::write(r.join("b.txt"), "BBB precious\n").unwrap();
    let out = run(&t.edit, json!({"input": format!("[a.txt#{}]\nMV b.txt\n", tag(r, "a.txt"))})).await;
    assert!(!out.is_error, "{}", text(&out));
    assert_eq!(std::fs::read_to_string(r.join("b.txt")).unwrap(), "AAA\n");
    assert!(!r.join("a.txt").exists());

    // A write failing after an earlier file landed names that file.
    std::fs::write(r.join("p1.txt"), "one\n").unwrap();
    std::fs::write(r.join("p2.txt"), "two\n").unwrap();
    std::fs::write(r.join("blocker"), "not a dir\n").unwrap();
    let payload =
        format!("[p1.txt#{}]\nPUT >1:\n+added\n[p2.txt#{}]\nMV blocker/p3.txt\n", tag(r, "p1.txt"), tag(r, "p2.txt"));
    let out = run(&t.edit, json!({"input": payload})).await;
    assert!(out.is_error);
    let msg = text(&out);
    assert!(msg.starts_with("Cannot write blocker/p3.txt:"), "{msg}");
    assert!(
        msg.ends_with("Already changed on disk by this edit before the failure (re-read before retrying): p1.txt"),
        "{msg}"
    );
    assert_eq!(std::fs::read_to_string(r.join("p1.txt")).unwrap(), "one\nadded\n");

    // A moved script keeps its executable bit.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(r.join("run.sh"), "#!/bin/sh\necho hi\n").unwrap();
        std::fs::set_permissions(r.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let out = run(&t.edit, json!({"input": format!("[run.sh#{}]\nMV bin/run.sh\n", tag(r, "run.sh"))})).await;
        assert!(!out.is_error, "{}", text(&out));
        assert_eq!(std::fs::metadata(r.join("bin/run.sh")).unwrap().permissions().mode() & 0o777, 0o755);
    }
}

#[tokio::test]
async fn hashline_write_notebooks_and_untaggable_reads() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    let ctx = ToolContext::new(r).with_read_output_max_columns(0.0);
    let write = WriteTool::new(ctx.clone());
    let read = ReadTool { ctx: ctx.clone() };
    let edit = EditTool::new(ctx.clone());
    // Original three WRITE header contracts in the established engine group.
    let module_content = "export const value = 42;\nexport const flag = true;\n";
    let module_result = run(&write, json!({"path":"module.ts","content":module_content})).await;
    let module_tag = ara_edit::store::file_hash(module_content);
    assert_eq!(text(&module_result), format!("[module.ts#{module_tag}]\nSuccessfully wrote {} bytes to module.ts", module_content.encode_utf16().count()));
    let module_key = ara_edit::path_policy::canonical_key(&r.join("module.ts"));
    assert_eq!(ctx.edit_store.by_hash(&module_key, &module_tag).unwrap().text.as_ref(), module_content);
    let config_content = "export const enabled = false;\n";
    let config_result = run(&write, json!({"path":"config.ts","content":config_content})).await;
    let config_header = text(&config_result).lines().next().unwrap().to_owned();
    let changed = run(&edit, json!({"input":format!("{config_header}\nPUT 1-1:\n+export const enabled = true;\n")})).await;
    assert!(!changed.is_error, "{}", text(&changed));
    assert_eq!(std::fs::read_to_string(r.join("config.ts")).unwrap(), "export const enabled = true;\n");
    let plain_write = WriteTool::new(ToolContext::new(r).with_edit(EditMode::Replace, true));
    let plain_result = run(&plain_write, json!({"path":"plain.txt","content":"no anchors here\n"})).await;
    assert_eq!(text(&plain_result), "Successfully wrote 16 bytes to plain.txt");
    // Copied read output: header path unwrapped, `N:` prefixes stripped, fresh tag returned.
    let out = run(&write, json!({"path": "[w.py#ABCD]", "content": "1:def f():\n2:    return 1\n"})).await;
    let body = "def f():\n    return 1\n";
    assert_eq!(std::fs::read_to_string(r.join("w.py")).unwrap(), body);
    assert!(!r.join("[w.py#ABCD]").exists());
    let fresh = ara_edit::store::file_hash(body);
    assert_eq!(
        text(&out),
        format!(
            "[w.py#{fresh}]\nSuccessfully wrote 22 bytes to w.py\nNote: auto-stripped hashline display prefixes from content before writing."
        )
    );
    // The returned tag anchors the next edit without a re-read.
    let out = run(&edit, json!({"input": format!("[w.py#{fresh}]\nPUT 2.=2:\n+    return 2\n")})).await;
    assert!(!out.is_error, "{}", text(&out));
    assert_eq!(std::fs::read_to_string(r.join("w.py")).unwrap(), "def f():\n    return 2\n");

    // Notebooks read as editable cells; the tag matches what edit sees.
    let nb = json!({
        "cells": [{"cell_type": "code", "execution_count": null, "metadata": {}, "outputs": [], "source": ["print(1)\n"]}],
        "metadata": {}, "nbformat": 4, "nbformat_minor": 5
    });
    std::fs::write(r.join("n.ipynb"), serde_json::to_string_pretty(&nb).unwrap()).unwrap();
    let shown = text(&run(&read, json!({"path": "n.ipynb"})).await);
    let header = shown.lines().next().unwrap().to_string();
    assert!(header.starts_with("[n.ipynb#") && !shown.contains("\"cells\""), "{shown}");
    let print_line = shown.lines().find(|l| l.ends_with(":print(1)")).unwrap().split(':').next().unwrap().to_string();
    let out = run(&edit, json!({"input": format!("{header}\nPUT {print_line}.={print_line}:\n+print(2)\n")})).await;
    assert!(!out.is_error, "{}", text(&out));
    assert!(std::fs::read_to_string(r.join("n.ipynb")).unwrap().contains("print(2)"));

    // An explicit one-line selector retains the 50KiB floor in the upgraded READ policy.
    std::fs::write(r.join("wide.txt"), format!("{}\nsecond\nthird\n", "x".repeat(60 * 1024))).unwrap();
    assert_eq!(
        text(&run(&read, json!({"path": "wide.txt:1-1"})).await),
        "[Line 1 is 60.0KB, exceeds 50.0KB limit. Hashline output requires full lines; cannot emit an editable numbered preview for a truncated line.]\n\n[2 more lines in file. Use :2 to continue]"
    );
    // New READ records the decoded whole-file snapshot after the binary sniff.
    // This display tag does not imply strict UTF-8 EDIT staging will accept the file.
    let mut bytes = "ok line\n".repeat(2000).into_bytes();
    bytes.extend_from_slice(b"bad \xff\n");
    let display_tag = ara_edit::store::file_hash(&String::from_utf8_lossy(&bytes));
    std::fs::write(r.join("bin.txt"), bytes).unwrap();
    assert_eq!(
        text(&run(&read, json!({"path": "bin.txt:1-2"})).await),
        format!("[bin.txt#{display_tag}]\n1:ok line\n2:ok line\n3:ok line\n4:ok line\n5:ok line\n\n[1996 more lines in file. Use :6 to continue]")
    );
}

#[tokio::test]
async fn engine_level_guards_cover_recovery_and_ordering() {
    if let Ok(mode) = std::env::var("ARA_FS_SCAN_OBSERVATION") {
        observe_scan_cache_configuration(&mode);
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    // Unique-suffix recovery cannot bypass the new strict UTF-8 staging guard.
    std::fs::create_dir_all(r.join("sub")).unwrap();
    let latin1 = b"caf\xe9\nthird\n".to_vec();
    std::fs::write(r.join("sub/legacy.txt"), &latin1).unwrap();
    let rt = tools(r, EditMode::Replace);
    let out = run(&rt.edit, json!({"path": "legacy.txt", "old_string": "third", "new_string": "THIRD"})).await;
    assert!(out.is_error, "{}", text(&out));
    assert!(text(&out).contains("not valid UTF-8"), "{}", text(&out));
    assert_eq!(std::fs::read(r.join("sub/legacy.txt")).unwrap(), latin1);
    // Deleting an invalid-byte file remains allowed without reencoding.
    std::fs::write(r.join("sub/legacy.txt"), &latin1).unwrap();
    let t = tools(r, EditMode::Hashline);
    let lossy_tag = ara_edit::store::file_hash(&String::from_utf8_lossy(&latin1));
    let out = run(&t.edit, json!({"input": format!("[sub/legacy.txt#{lossy_tag}]\nREM\n")})).await;
    assert!(!out.is_error, "{}", text(&out));
    assert!(!r.join("sub/legacy.txt").exists());

    // `REM b.txt` earlier in the payload frees the `MV` destination.
    std::fs::write(r.join("a.txt"), "new\n").unwrap();
    std::fs::write(r.join("b.txt"), "old\n").unwrap();
    let payload = format!("[b.txt#{}]\nREM\n[a.txt#{}]\nMV b.txt\n", tag(r, "b.txt"), tag(r, "a.txt"));
    let out = run(&t.edit, json!({"input": payload})).await;
    assert!(!out.is_error, "{}", text(&out));
    assert_eq!(std::fs::read_to_string(r.join("b.txt")).unwrap(), "new\n");
    assert!(!r.join("a.txt").exists());

    // Upstream single-window READ shows raw LF segments; the edit snapshot normalizes CR/BOM.
    std::fs::write(r.join("cr.txt"), "a\rb\nc\n").unwrap();
    let shown = text(&run(&t.read, json!({"path": "cr.txt"})).await);
    assert!(shown.ends_with("\n1:a\rb\n2:c"), "{shown}");
    let header = shown.lines().next().unwrap().to_string();
    let payload = json!({"input": format!("{header}\nPUT 3.=3:\n+C\n")});
    let out = run(&t.edit, payload.clone()).await;
    // b91 enforces the displayed rows: raw READ showed only rows 1 and 2,
    // while EDIT normalizes the lone CR and targets normalized row 3.
    assert!(out.is_error && text(&out).contains("never displayed"), "{}", text(&out));
    assert!(text(&out).contains("3:c"), "{}", text(&out));
    assert_eq!(std::fs::read_to_string(r.join("cr.txt")).unwrap(), "a\rb\nc\n");
    let retry = run(&t.edit, payload).await;
    assert!(!retry.is_error, "{}", text(&retry));
    assert!(std::fs::read_to_string(r.join("cr.txt")).unwrap().ends_with("C\n"), "the third line changed");
    std::fs::write(r.join("bom.txt"), "\u{feff}first\nsecond\n").unwrap();
    let shown = text(&run(&t.read, json!({"path": "bom.txt"})).await);
    assert!(shown.ends_with("\n1:\u{feff}first\n2:second"), "{shown:?}");
    observe_owned_scan_cache();
    // Static environment configuration is exercised in two isolated processes,
    // within this original coarse family, without changing process-global env.
    for mode in ["bounded-config", "disabled"] {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "engine_level_guards_cover_recovery_and_ordering", "--nocapture"])
            .env("ARA_FS_SCAN_OBSERVATION", mode)
            .env("ARA_WALK_WORKERS", "0")
            .env("ARA_FS_SCAN_CACHE_TTL_MS", if mode == "disabled" { "0" } else { "250" })
            .env("ARA_FS_SCAN_EMPTY_RECHECK_MS", "12")
            .env("ARA_FS_SCAN_CACHE_MAX_ENTRIES", "1");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut command = tokio::process::Command::from(command);
        command.kill_on_drop(true);
        let output = tokio::time::timeout(std::time::Duration::from_secs(5), command.output()).await.unwrap().unwrap();
        assert!(
            output.status.success(),
            "Fs cache {mode}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
    }
}

fn scan_budget() -> ara_walk::Budget {
    ara_walk::Budget {
        cancel: CancellationToken::new(),
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(3),
    }
}

fn cached_paths(root: &Path, pattern: &str) -> Vec<ara_tools::engine::GlobEntry> {
    ara_tools::engine::glob_with_cache(
        &ara_tools::engine::GlobRequest {
            root,
            pattern,
            recursive: true,
            include_hidden: true,
            use_gitignore: false,
            max_results: usize::MAX,
        },
        &scan_budget(),
        true,
    )
    .unwrap()
}

fn observe_owned_scan_cache() {
    use ara_walk::{CollectOptions, EntryKind, FollowLinks, WalkDetail, WalkError, WalkOptions, collect_entries};
    use std::time::{Duration, Instant};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    for path in [".git", "node_modules/pkg", "target", ".hidden-dir"] {
        std::fs::create_dir_all(root.join(path)).unwrap();
    }
    for (path, bytes) in [
        (".gitignore", "ignored.txt\n.ignored-hidden\n"),
        ("real.txt", "real"),
        ("ignored.txt", "ignore"),
        ("node_modules/pkg/index.js", "nm"),
        ("target/linked.txt", "linked"),
        (".hidden-file", "secret"),
        (".hidden-dir/child.txt", "child"),
        (".ignored-hidden", "ignore"),
    ] {
        std::fs::write(root.join(path), bytes).unwrap();
    }
    #[cfg(unix)]
    {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let fifo = CString::new(root.join("skip-me.fifo").as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(
            !collect_entries(
                root,
                CollectOptions {
                    walk: WalkOptions {
                        include_hidden: true,
                        use_gitignore: false,
                        skip_node_modules: false,
                        max_depth: usize::MAX
                    },
                    detail: WalkDetail::Full,
                    follow_links: FollowLinks::Never,
                    cache: false,
                },
                &scan_budget()
            )
            .unwrap()
            .entries
            .iter()
            .any(|entry| entry.relative == "skip-me.fifo")
        );
    }
    let base = CollectOptions {
        walk: WalkOptions {
            include_hidden: false,
            use_gitignore: true,
            skip_node_modules: true,
            max_depth: usize::MAX,
        },
        detail: WalkDetail::Minimal,
        follow_links: FollowLinks::Never,
        cache: true,
    };
    let minimal = collect_entries(root, base, &scan_budget()).unwrap();
    assert!(minimal.entries.iter().any(|entry| entry.relative == "real.txt"));
    assert!(!minimal.entries.iter().any(|entry| entry.relative == "ignored.txt"
        || entry.relative.starts_with('.')
        || entry.relative.contains("node_modules")));
    assert!(minimal.entries.iter().all(|entry| entry.mtime_ms.is_none() && entry.size.is_none()));
    let full = collect_entries(root, CollectOptions { detail: WalkDetail::Full, ..base }, &scan_budget()).unwrap();
    let real = full.entries.iter().find(|entry| entry.relative == "real.txt").unwrap();
    assert_eq!(real.kind, EntryKind::File);
    assert_eq!(real.size, Some(4));
    assert!(real.mtime_ms.is_some());
    let target = full.entries.iter().find(|entry| entry.relative == "target").unwrap();
    assert_eq!(target.kind, EntryKind::Dir);
    assert!(target.mtime_ms.is_some() && target.size.is_none());
    let hidden = collect_entries(
        root,
        CollectOptions { walk: WalkOptions { include_hidden: true, ..base.walk }, detail: WalkDetail::Full, ..base },
        &scan_budget(),
    )
    .unwrap();
    assert!(hidden.entries.iter().any(|entry| entry.relative == ".hidden-file"));
    assert!(hidden.entries.iter().any(|entry| entry.relative == ".hidden-dir/child.txt"));
    assert!(
        !hidden.entries.iter().any(|entry| entry.relative == ".ignored-hidden" || entry.relative.starts_with(".git/"))
    );
    let unignored = collect_entries(
        root,
        CollectOptions { walk: WalkOptions { use_gitignore: false, skip_node_modules: false, ..base.walk }, ..base },
        &scan_budget(),
    )
    .unwrap();
    assert!(unignored.entries.iter().any(|entry| entry.relative == "ignored.txt"));
    assert!(unignored.entries.iter().any(|entry| entry.relative == "node_modules/pkg/index.js"));
    let shallow = collect_entries(
        root,
        CollectOptions { walk: WalkOptions { max_depth: 1, ..base.walk }, ..base },
        &scan_budget(),
    )
    .unwrap();
    assert!(!shallow.entries.iter().any(|entry| entry.relative.contains('/')));
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join("target"), root.join("link")).unwrap();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let output = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(root.join("link"))
            .arg(root.join("target"))
            .creation_flags(0x0800_0000)
            .output()
            .unwrap();
        assert!(output.status.success(), "junction fixture: {}", String::from_utf8_lossy(&output.stderr));
    }
    ara_walk::invalidate_path(root);
    let never = collect_entries(root, base, &scan_budget()).unwrap();
    assert!(!never.entries.iter().any(|entry| entry.relative == "link/linked.txt"));
    let follow =
        collect_entries(root, CollectOptions { follow_links: FollowLinks::Always, ..base }, &scan_budget()).unwrap();
    assert!(follow.entries.iter().any(|entry| entry.relative == "link/linked.txt"));
    assert!(collect_entries(&root.join("link"), base, &scan_budget()).unwrap().entries.is_empty());
    let roots = collect_entries(
        &root.join("link"),
        CollectOptions { follow_links: FollowLinks::Roots, ..base },
        &scan_budget(),
    )
    .unwrap();
    assert!(roots.entries.iter().any(|entry| entry.relative == "linked.txt"));
    assert!(
        cached_paths(&root.join("link"), "*.txt").iter().any(|entry| entry.relative == "linked.txt"),
        "native glob resolves its root link while preserving Never for descendant links"
    );
    let cancelled = scan_budget();
    cancelled.cancel.cancel();
    assert_eq!(
        collect_entries(root, base, &cancelled).unwrap_err(),
        WalkError::Aborted,
        "cached results cannot bypass cancellation"
    );
    let expired =
        ara_walk::Budget { cancel: CancellationToken::new(), deadline: Instant::now() - Duration::from_millis(1) };
    assert_eq!(collect_entries(root, base, &expired).unwrap_err(), WalkError::Timeout);

    // Relative invalidation resolves against the actual process cwd, with no
    // process-global cwd mutation that could interfere with other coarse cases.
    let cwd = std::env::current_dir().unwrap();
    let relative_directory = tempfile::Builder::new().prefix("ara-relative-cache-").tempdir_in(&cwd).unwrap();
    let first = relative_directory.path().join("first");
    let second = relative_directory.path().join("second");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    std::fs::write(first.join("existing.txt"), "first").unwrap();
    std::fs::write(second.join("existing.txt"), "second").unwrap();
    let started = Instant::now();
    let _ = cached_paths(&first, "*.txt");
    let _ = cached_paths(&second, "*.txt");
    std::fs::write(first.join("late.txt"), "late").unwrap();
    std::fs::write(second.join("late.txt"), "late").unwrap();
    assert!(!cached_paths(&first, "*.txt").iter().any(|entry| entry.relative == "late.txt"));
    let default_request = ara_tools::engine::GlobRequest {
        root: &first,
        pattern: "*.txt",
        recursive: true,
        include_hidden: true,
        use_gitignore: false,
        max_results: usize::MAX,
    };
    assert!(
        ara_tools::engine::glob(&default_request, &scan_budget())
            .unwrap()
            .iter()
            .any(|entry| entry.relative == "late.txt"),
        "ordinary glob remains a fresh visitor consumer"
    );
    assert!(
        !cached_paths(&first, "*.txt").iter().any(|entry| entry.relative == "late.txt"),
        "cache=false did not update the shared raw scan"
    );
    let relative = first.join("late.txt").strip_prefix(&cwd).unwrap().to_path_buf();
    ara_walk::invalidate_path_string(relative.to_str().unwrap());
    assert!(cached_paths(&first, "*.txt").iter().any(|entry| entry.relative == "late.txt"));
    assert!(
        !cached_paths(&second, "*.txt").iter().any(|entry| entry.relative == "late.txt"),
        "path invalidation preserves unrelated roots"
    );
    std::fs::remove_file(first.join("late.txt")).unwrap();
    ara_walk::invalidate_path(&first.join("late.txt"));
    assert!(
        !cached_paths(&first, "*.txt").iter().any(|entry| entry.relative == "late.txt"),
        "deleted path canonical-parent fallback invalidates its root"
    );
    ara_tools::engine::invalidate_fs_scan_cache(None);
    assert!(cached_paths(&second, "*.txt").iter().any(|entry| entry.relative == "late.txt"));
    assert!(started.elapsed() < Duration::from_millis(ara_walk::cache_ttl_ms()));
    for (name, seconds) in [("rank-a.txt", 10), ("rank-b.txt", 20), ("rank-c.txt", 20)] {
        let file = std::fs::File::create(first.join(name)).unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH + Duration::from_secs(seconds)))
            .unwrap();
    }
    ara_walk::invalidate_path(&first);
    let ranked = ara_tools::engine::GlobRequest { pattern: "rank-*.txt", max_results: 2, ..default_request };
    assert_eq!(
        ara_tools::engine::glob_with_cache(&ranked, &scan_budget(), true)
            .unwrap()
            .iter()
            .map(|entry| entry.relative.as_str())
            .collect::<Vec<_>>(),
        vec!["rank-b.txt", "rank-c.txt"],
        "cached filtering preserves mtime descending, path tie ordering, then limit"
    );
    eprintln!(
        "Owned Fs scan receipt: {}",
        json!({"pid":std::process::id(),"fullEntries":full.entries.iter().map(|entry|json!({"path":entry.relative,"kind":format!("{:?}",entry.kind),"mtime":entry.mtime_ms,"size":entry.size})).collect::<Vec<_>>(),
        "followPaths":follow.entries.iter().map(|entry|&entry.relative).collect::<Vec<_>>(),"relativeInvalidate":relative,"pathSelective":true,"globalInvalidate":true,
        "originalIds":["B-566ed432f8","B-226dd04ca4","B-5bc9376a1f","B-038e6d0f44","B-2c4efa9c64","B-0409f4ed11","B-3a4fade42b","B-913dc4f4ce","B-8593194f43","B-6dc8963eb8"],
        "fifoRuntime":cfg!(unix),"boundary":"Windows junction exercises follow-links; Unix FIFO execution remains OS-specific; full parallel walker parent OPEN"})
    );
}

fn observe_scan_cache_configuration(mode: &str) {
    use std::time::{Duration, Instant};
    assert_eq!(ara_walk::walk_workers(), std::thread::available_parallelism().map_or(4, usize::from));
    assert_eq!(ara_walk::max_cache_entries(), 1);
    assert_eq!(ara_walk::empty_recheck_ms(), 12);
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("first");
    let second = directory.path().join("second");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    std::fs::write(first.join("keep.txt"), "keep").unwrap();
    std::fs::write(second.join("keep.txt"), "keep").unwrap();
    let _ = cached_paths(&first, "*.txt");
    if mode == "disabled" {
        assert_eq!(ara_walk::cache_ttl_ms(), 0);
        std::fs::write(first.join("late.txt"), "late").unwrap();
        assert!(cached_paths(&first, "*.txt").iter().any(|entry| entry.relative == "late.txt"));
    } else {
        assert_eq!(ara_walk::cache_ttl_ms(), 250);
        let seeded = Instant::now();
        ara_walk::invalidate_path(&first);
        let _ = cached_paths(&first, "*.txt");
        std::fs::write(first.join("late.rs"), "late").unwrap();
        // The raw scan is nonempty, but filtering its cached entries is empty.
        assert!(cached_paths(&first, "*.rs").is_empty());
        std::thread::sleep(Duration::from_millis(15));
        assert!(seeded.elapsed() < Duration::from_millis(250), "recheck observation must precede TTL expiry");
        assert_eq!(
            cached_paths(&first, "*.rs").iter().map(|entry| entry.relative.as_str()).collect::<Vec<_>>(),
            vec!["late.rs"]
        );
        let _ = cached_paths(&second, "*.txt"); // max=1 evicts the first raw root.
        std::fs::write(first.join("eviction.txt"), "eviction").unwrap();
        assert!(cached_paths(&first, "*.txt").iter().any(|entry| entry.relative == "eviction.txt"));
        std::fs::write(first.join("expired.txt"), "expired").unwrap();
        assert!(!cached_paths(&first, "*.txt").iter().any(|entry| entry.relative == "expired.txt"));
        std::thread::sleep(Duration::from_millis(255));
        assert!(cached_paths(&first, "*.txt").iter().any(|entry| entry.relative == "expired.txt"));
    }
    eprintln!(
        "Fs cache configuration receipt: {}",
        json!({"mode":mode,"pid":std::process::id(),"workersAuto":ara_walk::walk_workers(),
        "ttlMs":ara_walk::cache_ttl_ms(),"emptyRecheckMs":ara_walk::empty_recheck_ms(),"maxEntries":ara_walk::max_cache_entries(),
        "actualPaths":cached_paths(&first,"*").iter().map(|entry|&entry.relative).collect::<Vec<_>>()})
    );
}

#[tokio::test]
async fn write_stripping_gate_and_notebook_tags() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    let ctx = ToolContext::new(r);
    let write = WriteTool::new(ctx.clone());
    let edit = EditTool::new(ctx);
    // Fixed OMP write.ts:332-363 strips every consistently prefixed row; the former
    // ARA consecutive/lone-row gate was a custom deviation, restored for parity.
    let yaml = "200: OK\n404: Not Found\n";
    let out = run(&write, json!({"path": "codes.yaml", "content": yaml})).await;
    assert_eq!(std::fs::read_to_string(r.join("codes.yaml")).unwrap(), " OK\n Not Found\n");
    assert!(text(&out).contains("auto-stripped"));
    let out = run(&write, json!({"path": "standup.txt", "content": "10:30 standup"})).await;
    assert_eq!(std::fs::read_to_string(r.join("standup.txt")).unwrap(), "30 standup");
    assert!(text(&out).contains("auto-stripped"));
    // A notebook write returns a tag of its editable cells, usable by edit.
    let nb = json!({
        "cells": [{"cell_type": "code", "execution_count": null, "metadata": {}, "outputs": [], "source": ["x = 1\n"]}],
        "metadata": {}, "nbformat": 4, "nbformat_minor": 5
    });
    let out = run(&write, json!({"path": "n.ipynb", "content": serde_json::to_string_pretty(&nb).unwrap()})).await;
    let header = text(&out).lines().next().unwrap().to_string();
    let cells =
        ara_edit::notebook::notebook_to_editable_text(&std::fs::read_to_string(r.join("n.ipynb")).unwrap(), "n.ipynb")
            .unwrap();
    let line = cells.lines().position(|l| l == "x = 1").unwrap() + 1;
    let out = run(&edit, json!({"input": format!("{header}\nPUT {line}.={line}:\n+x = 2\n")})).await;
    assert!(!out.is_error, "{}", text(&out));
    assert!(std::fs::read_to_string(r.join("n.ipynb")).unwrap().contains("x = 2"));
}

/// Four fixed edit-tool-details cases plus the actual post-write noop case,
/// grouped at the existing filesystem EditTool module entrance.
#[tokio::test]
async fn fixed_details_inspection_and_post_write_unchanged_family() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    std::fs::write(r.join("update.txt"), "old\n").unwrap();
    std::fs::write(r.join("move.txt"), "move me\n").unwrap();
    let input = [
        "*** Begin Patch",
        "*** Update File: update.txt",
        "@@",
        "-old",
        "+new",
        "*** Update File: move.txt",
        "*** Move to: moved.txt",
        "@@",
        "-move me",
        "+moved",
        "*** Add File: created.txt",
        "+created",
        "*** End Patch",
        "",
    ]
    .join("\n");
    let result = run(&tools(r, EditMode::ApplyPatch).edit, json!({"input":input})).await;
    assert!(!result.is_error, "{}", text(&result));
    let details = result.details.unwrap();
    let files = details["perFileResults"].as_array().unwrap();
    assert_eq!(files.len(), 3);
    for (index, path, op, before, after) in [
        (0, "update.txt", "update", Some("old\n"), "new\n"),
        (1, "moved.txt", "update", Some("move me\n"), "moved\n"),
        (2, "created.txt", "create", None, "created\n"),
    ] {
        let file = &files[index];
        assert_eq!(file["path"], json!(r.join(path).to_string_lossy()));
        assert_eq!(file["firstChangedLine"], 1);
        assert_eq!(file["op"], op);
        assert_eq!(file.get("oldText").and_then(Value::as_str), before);
        assert_eq!(file["newText"], after);
        assert!(file["diff"].as_str().unwrap().contains(&format!("+1|{}", after.trim_end())));
        assert_eq!(std::fs::read_to_string(r.join(path)).unwrap(), after);
    }
    assert_eq!(files[1]["move"], json!(r.join("moved.txt").to_string_lossy()));
    assert_eq!(files[1]["sourcePath"], json!(r.join("move.txt").to_string_lossy()));
    assert!(!r.join("move.txt").exists());

    let context = ToolContext::new(r).with_edit(EditMode::Hashline, true);
    let mut sections = Vec::new();
    for (name, before, after) in [("a.ts", "a\n", "A"), ("b.ts", "b\n", "B")] {
        std::fs::write(r.join(name), before).unwrap();
        let tag = context.edit_store.record(&r.join(name), before, None);
        sections.push(format!("[{name}#{tag}]\nPUT 1-1:\n+{after}"));
    }
    let result = run(&EditTool::new(context), json!({"input":sections.join("\n")})).await;
    assert!(!result.is_error, "{}", text(&result));
    let details = result.details.unwrap();
    let files = details["perFileResults"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    for (index, name, before, after) in [(0, "a.ts", "a\n", "A\n"), (1, "b.ts", "b\n", "B\n")] {
        assert_eq!(files[index]["path"], json!(r.join(name).to_string_lossy()));
        assert_eq!(files[index]["firstChangedLine"], 1);
        assert_eq!(files[index]["op"], "update");
        assert_eq!(files[index]["oldText"], before);
        assert_eq!(files[index]["newText"], after);
        assert!(files[index]["diff"].as_str().unwrap().contains(&format!("+1|{}", after.trim_end())));
    }

    let before = format!("{}old\n", "line\n".repeat(9_000));
    std::fs::write(r.join("large.txt"), before).unwrap();
    let result = run(
        &tools(r, EditMode::Patch).edit,
        json!({"path":"large.txt", "edits":[{"op":"update","diff":"@@\n-old\n+new"}]}),
    )
    .await;
    assert!(!result.is_error, "{}", text(&result));
    let details = result.details.unwrap();
    assert_eq!(details["path"], json!(r.join("large.txt").to_string_lossy()));
    assert!(details["diff"].as_str().unwrap().contains("+9001|new"));
    assert_eq!(details["firstChangedLine"], 9_001);
    assert_eq!(details["snapshotsPruned"], true);
    assert!(details.get("oldText").is_none());
    assert!(details.get("newText").is_none());

    std::fs::write(r.join("warning.ts"), "function greet() {\n  return 'hello';\n}\n").unwrap();
    let result = run(&tools(r, EditMode::Patch).edit, json!({"path":"warning.ts", "edits":[{"op":"update", "diff":"@@\n function greet() {\n-  return 'hell0';\n+  return 'bye';\n }"}]})).await;
    assert!(!result.is_error, "{}", text(&result));
    let details = result.details.unwrap();
    assert_eq!(details["diagnostics"]["server"], "patch");
    assert_eq!(details["diagnostics"]["summary"], "Patch warnings: 1");
    assert_eq!(details["diagnostics"]["errored"], false);
    let messages = details["diagnostics"]["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].as_str().unwrap().starts_with("patch: Inexact match in warning.ts near line 1:"));
    assert_eq!(details["meta"]["diagnostics"]["summary"], details["diagnostics"]["summary"]);
    assert_eq!(details["meta"]["diagnostics"]["messages"], details["diagnostics"]["messages"]);

    // Existing FsWriter guard runs after the Host callback reports success.
    // The real file stays unchanged; this is not a pure helper assertion.
    struct NoopUpdate;
    impl ara_tools::edit::EditUpdateWriter for NoopUpdate {
        fn write(&self, _: &Path, _: &str) -> std::io::Result<()> {
            Ok(())
        }
    }
    let relative = "deep/nested/foo.txt";
    std::fs::create_dir_all(r.join("deep/nested")).unwrap();
    std::fs::write(r.join(relative), "a\n").unwrap();
    let tool =
        EditTool::new(ToolContext::new(r).with_edit(EditMode::Patch, true)).with_update_writer(Arc::new(NoopUpdate));
    let result = run(&tool, json!({"path":relative, "edits":[{"op":"update", "diff":"@@\n-a\n+b"}]})).await;
    assert!(result.is_error);
    assert!(
        text(&result)
            .contains(&format!("edit appeared successful but file content did not change on disk: {relative}"))
    );
    assert!(!text(&result).contains(r.to_string_lossy().as_ref()));
    assert_eq!(std::fs::read_to_string(r.join(relative)).unwrap(), "a\n");
}

/// Fixed EditTool.openArgStream pool and same-call execute source contracts.
/// Grouped with existing edit module, not a parallel fake edit runtime.
#[tokio::test]
async fn streamed_session_reuse_pool_replacement_eviction_and_generated_error_group() {
    use ara_agent::ToolArgStreamInit;
    use std::sync::{Mutex, atomic::{AtomicBool, Ordering}};
    // The existing independent effect runs this writer on a blocking thread.
    // Gate one settled disk write so replacement happens during a real apply,
    // rather than merely after borrowing a not-yet-entered EditCall handle.
    struct EnteredWriter {
        armed: AtomicBool,
        entered: tokio::sync::Notify,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
    }
    impl ara_tools::edit::EditUpdateWriter for EnteredWriter {
        fn write(&self, path: &Path, content: &str) -> std::io::Result<()> {
            std::fs::write(path, content)?;
            if self.armed.swap(false, Ordering::SeqCst) {
                self.entered.notify_one();
                self.release.lock().unwrap().recv_timeout(std::time::Duration::from_secs(12))
                    .map_err(|error| std::io::Error::new(std::io::ErrorKind::TimedOut, error))?;
            }
            Ok(())
        }
    }
    let (release, receiver) = std::sync::mpsc::sync_channel(1);
    let writer = Arc::new(EnteredWriter { armed: AtomicBool::new(false),
        entered: tokio::sync::Notify::new(), release: Mutex::new(receiver) });
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.txt"), "alpha\n").unwrap();
    let tool = EditTool::new(ToolContext::new(d.path()).with_edit(EditMode::Replace, true))
        .with_update_writer(writer.clone());
    let batches = Arc::new(Mutex::new(Vec::new()));
    let open = |id: &str| {
        let batches = batches.clone();
        tool.open_arg_stream(ToolArgStreamInit {
            tool_call_id: id.into(),
            tool_name: "edit".into(),
            custom_wire_name: None,
            emit: Arc::new(move |batch| batches.lock().unwrap().push(batch)),
        })
        .unwrap()
        .unwrap()
    };
    let payload = json!({"path":"a.txt","old_string":"alpha","new_string":"beta"});
    let mut stream = open("same");
    for delta in payload.to_string().as_bytes().chunks(7) {
        stream.push(std::str::from_utf8(delta).unwrap()).unwrap();
    }
    // end must not overwrite the raw buffer with provider-final arguments.
    stream.end(&args(json!({"path":"wrong.txt","old_string":"other","new_string":"lost"}))).unwrap();
    wait_edit_preview(&batches).await;
    assert_eq!(std::fs::read_to_string(d.path().join("a.txt")).unwrap(), "alpha\n");
    assert_eq!(batches.lock().unwrap().last().unwrap()["streaming"], false);
    let result = tool.execute("same", args(payload.clone()), CancellationToken::new(), Arc::new(|_| {})).await.unwrap();
    assert!(!result.is_error, "{}", text(&result));
    assert_eq!(
        std::fs::read_to_string(d.path().join("a.txt")).unwrap(),
        "beta\n",
        "effective execute arguments win when end metadata differs from the raw preview"
    );
    stream.cancel().unwrap();

    let mut old = open("replacement");
    let mut current = open("replacement");
    old.cancel().unwrap();
    batches.lock().unwrap().clear();
    let replacement_payload = json!({"path":"a.txt","old_string":"beta","new_string":"gamma"});
    current.push(&replacement_payload.to_string()).unwrap();
    current.end(&args(replacement_payload.clone())).unwrap();
    wait_edit_preview(&batches).await;
    let result =
        tool.execute("replacement", args(replacement_payload), CancellationToken::new(), Arc::new(|_| {})).await.unwrap();
    assert!(!result.is_error, "old cancel cannot remove the new same-ID session");
    assert_eq!(std::fs::read_to_string(d.path().join("a.txt")).unwrap(), "gamma\n");

    let mut oldest = open("oldest");
    let mut held = Vec::new();
    for index in 0..32 {
        held.push(open(&format!("pool-{index}")));
    }
    let count = batches.lock().unwrap().len();
    oldest.push(&payload.to_string()).unwrap();
    oldest.end(&JsonObject::new()).unwrap();
    assert_eq!(batches.lock().unwrap().len(), count, "33rd insertion closes the oldest of the 32-entry pool");
    oldest.cancel().unwrap();
    batches.lock().unwrap().clear();
    let pool_payload = json!({"path":"a.txt","old_string":"gamma","new_string":"delta"});
    held[0].push(&pool_payload.to_string()).unwrap();
    held[0].end(&args(pool_payload.clone())).unwrap();
    wait_edit_preview(&batches).await;
    assert!(
        !tool.execute("pool-0", args(pool_payload), CancellationToken::new(), Arc::new(|_| {})).await.unwrap().is_error,
        "cancel of evicted oldest preserves current row"
    );
    for receiver in &mut held {
        receiver.cancel().unwrap();
    }

    // An entered old invocation's finally must also preserve a replacement.
    let mut old = open("finally");
    batches.lock().unwrap().clear();
    old.push(&json!({"path":"a.txt","old_string":"delta","new_string":"epsilon"}).to_string()).unwrap();
    old.end(&JsonObject::new()).unwrap();
    wait_edit_preview(&batches).await;
    let old_call = tool.begin_streamed_call("finally").unwrap();
    writer.armed.store(true, Ordering::SeqCst);
    let old_effect = tool.execute_os_call(old_call, CancellationToken::new());
    tokio::pin!(old_effect);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::select! {
            _ = &mut old_effect => panic!("old apply settled before its gated write"),
            _ = writer.entered.notified() => {},
        }
    }).await.unwrap();
    assert_eq!(std::fs::read_to_string(d.path().join("a.txt")).unwrap(), "epsilon\n",
        "the original real effect entered before the same-ID replacement");
    let mut fresh = open("finally");
    batches.lock().unwrap().clear();
    let fresh_payload = json!({"path":"a.txt","old_string":"epsilon","new_string":"zeta"});
    fresh.push(&fresh_payload.to_string()).unwrap();
    fresh.end(&args(fresh_payload.clone())).unwrap();
    wait_edit_preview(&batches).await;
    release.send(()).unwrap();
    let old_result = tokio::time::timeout(std::time::Duration::from_secs(5), &mut old_effect).await.unwrap().unwrap();
    assert!(!old_result.is_error, "{}", text(&old_result));
    assert!(
        !tool.execute("finally", args(fresh_payload), CancellationToken::new(), Arc::new(|_| {})).await.unwrap().is_error
    );
    assert_eq!(std::fs::read_to_string(d.path().join("a.txt")).unwrap(), "zeta\n");
    old.cancel().unwrap();
    fresh.cancel().unwrap();

    let raw = EditTool::new(ToolContext::new(d.path()).with_edit(EditMode::ApplyPatch, true));
    let raw_batches = Arc::new(Mutex::new(Vec::new()));
    let capture = raw_batches.clone();
    let mut raw_stream = raw
        .open_arg_stream(ToolArgStreamInit {
            tool_call_id: "raw".into(),
            tool_name: "edit".into(),
            custom_wire_name: Some("apply_patch".into()),
            emit: Arc::new(move |batch| capture.lock().unwrap().push(batch)),
        })
        .unwrap()
        .unwrap();
    let raw_input = "*** Begin Patch\n*** Update File: a.txt\n@@\n-zeta\n+eta\n*** End Patch";
    let raw_payload = json!({"input": raw_input});
    raw_stream.push(raw_input).unwrap();
    raw_stream.end(&args(raw_payload.clone())).unwrap();
    wait_edit_preview(&raw_batches).await;
    assert_eq!(std::fs::read_to_string(d.path().join("a.txt")).unwrap(), "zeta\n");
    assert!(!raw.execute("raw", args(raw_payload), CancellationToken::new(), Arc::new(|_| {})).await.unwrap().is_error);
    assert_eq!(std::fs::read_to_string(d.path().join("a.txt")).unwrap(), "eta\n", "custom wire must use rawInput=true");
    assert_eq!(raw_batches.lock().unwrap().last().unwrap()["streaming"], false);
    raw_stream.cancel().unwrap();

    // B-1f5a4919eb: complete call staging rejects a generated multi-entry edit
    // once, preserves both original bytes and never publishes a proposed diff.
    let generated = "// Code generated by sqlc. DO NOT EDIT.\nexport const foo = 1;\nexport const bar = 2;\n";
    std::fs::write(d.path().join("generated.ts"), generated).unwrap();
    let patch = EditTool::new(ToolContext::new(d.path()).with_edit(EditMode::Patch, true));
    let output = run(
        &patch,
        json!({"path":"generated.ts","edits":[
            {"op":"update","diff":"@@\n-export const foo = 1;\n+export const foo = 11;\n"},
            {"op":"update","diff":"@@\n-export const bar = 2;\n+export const bar = 22;\n"}
        ]}),
    )
    .await;
    assert!(output.is_error);
    assert_eq!(std::fs::read_to_string(d.path().join("generated.ts")).unwrap(), generated);
    assert_eq!(text(&output).matches("Cannot modify auto-generated file").count(), 1);
    assert_eq!(
        output.details.as_ref().and_then(|details| details.get("diff")).and_then(Value::as_str).unwrap_or(""),
        ""
    );
}

async fn wait_edit_preview(batches: &Arc<std::sync::Mutex<Vec<Value>>>) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if batches.lock().unwrap().last().is_some_and(|batch| batch["streaming"] == false) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}

/// New upstream Host URL handshake. Controlled ports authorize real temporary
/// files; these tests do not claim an external editor/server acceptance.
mod host_url_upgrade {
    use super::*;
    use ara_tools::edit_host::{EditHostPathPolicy, EditHostPort};
    use ara_tools::{
        ContentUriEditPolicy, ContentUriEditSpec, ContentUriPort, ContentUriRoute, UriFileResource, UriResource,
    };
    use async_trait::async_trait;
    use std::{
        collections::HashMap,
        path::PathBuf,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };
    use tokio::sync::Notify;

    struct State {
        targets: HashMap<String, PathBuf>,
        policy: ContentUriEditPolicy,
        immutable: bool,
        resource_immutable: bool,
    }
    struct Port {
        state: Mutex<State>,
        calls: AtomicUsize,
        urls: Mutex<Vec<String>>,
        delay_first: bool,
        entered: Notify,
        release: Notify,
    }
    impl Port {
        fn new(path: PathBuf, delay_first: bool) -> Arc<Self> {
            Arc::new(Self {
                state: Mutex::new(State {
                    targets: HashMap::from([("local://target".into(), path)]),
                    policy: ContentUriEditPolicy::File { plan_writable: true },
                    immutable: false,
                    resource_immutable: false,
                }),
                calls: AtomicUsize::new(0),
                urls: Mutex::new(Vec::new()),
                delay_first,
                entered: Notify::new(),
                release: Notify::new(),
            })
        }
        fn tool(self: &Arc<Self>, root: &Path, mode: EditMode) -> EditTool {
            EditTool::new(ToolContext::new(root).with_edit(mode, true).with_uri_port(self.clone()))
        }
    }
    #[async_trait]
    impl ContentUriPort for Port {
        fn route(&self, url: &str) -> ContentUriRoute {
            if url.starts_with("local://") {
                ContentUriRoute::Registered { writable: true }
            } else {
                ContentUriRoute::Unregistered
            }
        }
        fn edit_specs(&self) -> Vec<ContentUriEditSpec> {
            let state = self.state.lock().unwrap();
            vec![ContentUriEditSpec {
                scheme: "local".into(),
                single_slash_alias: true,
                line_selectors: true,
                port_authority: false,
                immutable: state.immutable,
                policy: state.policy,
            }]
        }
        async fn resolve_edit_file(
            &self,
            url: &str,
            cancel: CancellationToken,
        ) -> Result<Option<UriFileResource>, ara_agent::ToolError> {
            let (path, immutable) = {
                let state = self.state.lock().unwrap();
                (state.targets.get(url).cloned(), state.resource_immutable)
            };
            self.urls.lock().unwrap().push(url.into());
            let index = self.calls.fetch_add(1, Ordering::AcqRel);
            if self.delay_first && index == 0 {
                self.entered.notify_one();
                tokio::select! {biased; _=cancel.cancelled()=>return Err(ara_agent::ToolError("resolver cancelled".into())),_=self.release.notified()=>{}}
            }
            if cancel.is_cancelled() {
                return Err(ara_agent::ToolError("resolver cancelled".into()));
            }
            Ok(path.map(|path| UriFileResource {
                path,
                content_type: "text/plain".into(),
                max_inline_bytes: None,
                immutable,
            }))
        }
        async fn read(&self, _: &str, _: CancellationToken) -> Result<UriResource, ara_agent::ToolError> {
            panic!("edit must not call handler read")
        }
        async fn write(&self, _: &str, _: &str, _: CancellationToken) -> Result<(), ara_agent::ToolError> {
            panic!("edit must not call handler write")
        }
    }
    fn replacement(path: &str) -> Value {
        json!({"path":path,"old_string":"alpha","new_string":"beta"})
    }
    async fn final_preview(tool: &EditTool, id: &str, payload: Value) -> Box<dyn ara_agent::ToolArgStream> {
        let (send, mut recv) = tokio::sync::mpsc::unbounded_channel();
        let mut stream = tool
            .open_arg_stream(ara_agent::ToolArgStreamInit {
                tool_call_id: id.into(),
                tool_name: "edit".into(),
                custom_wire_name: None,
                emit: Arc::new(move |batch| {
                    let _ = send.send(batch);
                }),
            })
            .unwrap()
            .unwrap();
        stream.push(&payload.to_string()).unwrap();
        stream.end(&JsonObject::new()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while let Some(batch) = recv.recv().await {
                if batch["streaming"] == false {
                    return;
                }
            }
            panic!("preview closed without final batch");
        })
        .await
        .unwrap();
        stream
    }

    #[tokio::test]
    async fn preview_apply_refreshes_file_grant_and_rechecks_refusal() {
        let d = tempfile::tempdir().unwrap();
        let stale = d.path().join("stale.txt");
        let fresh = d.path().join("fresh.txt");
        std::fs::write(&stale, "alpha\n").unwrap();
        std::fs::write(&fresh, "alpha\n").unwrap();
        let port = Port::new(stale.clone(), false);
        let tool = port.tool(d.path(), EditMode::Replace);
        let _stream = final_preview(&tool, "refresh", replacement("local:/target")).await;
        port.state.lock().unwrap().targets.insert("local://target".into(), fresh.clone());
        let result =
            tool.execute("refresh", JsonObject::new(), CancellationToken::new(), Arc::new(|_| {})).await.unwrap();
        assert!(!result.is_error, "{}", text(&result));
        assert_eq!(std::fs::read_to_string(&stale).unwrap(), "alpha\n");
        assert_eq!(std::fs::read_to_string(&fresh).unwrap(), "beta\n");
        assert_eq!(&*port.urls.lock().unwrap(), &["local://target", "local://target"]);
        std::fs::write(&fresh, "alpha\n").unwrap();
        let _stream = final_preview(&tool, "revoke", replacement("local://target")).await;
        port.state.lock().unwrap().policy = ContentUriEditPolicy::Handler;
        let result =
            tool.execute("revoke", JsonObject::new(), CancellationToken::new(), Arc::new(|_| {})).await.unwrap();
        assert!(result.is_error);
        assert!(text(&result).contains("written through `write`"));
        assert_eq!(std::fs::read_to_string(&fresh).unwrap(), "alpha\n");
    }

    #[tokio::test]
    async fn late_preview_cannot_authorize_or_overwrite_apply() {
        let d = tempfile::tempdir().unwrap();
        let stale = d.path().join("stale.txt");
        let fresh = d.path().join("fresh.txt");
        std::fs::write(&stale, "alpha\n").unwrap();
        std::fs::write(&fresh, "alpha\n").unwrap();
        let port = Port::new(stale.clone(), true);
        let tool = port.tool(d.path(), EditMode::Replace);
        let batches = Arc::new(AtomicUsize::new(0));
        let capture = batches.clone();
        let mut stream = tool
            .open_arg_stream(ara_agent::ToolArgStreamInit {
                tool_call_id: "late".into(),
                tool_name: "edit".into(),
                custom_wire_name: None,
                emit: Arc::new(move |_| {
                    capture.fetch_add(1, Ordering::AcqRel);
                }),
            })
            .unwrap()
            .unwrap();
        stream.push(&replacement("local://target").to_string()).unwrap();
        stream.end(&JsonObject::new()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), port.entered.notified()).await.unwrap();
        port.state.lock().unwrap().targets.insert("local://target".into(), fresh.clone());
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tool.execute("late", JsonObject::new(), CancellationToken::new(), Arc::new(|_| {})),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!result.is_error, "{}", text(&result));
        port.release.notify_one();
        assert_eq!(std::fs::read_to_string(stale).unwrap(), "alpha\n");
        assert_eq!(std::fs::read_to_string(fresh).unwrap(), "beta\n");
        assert_eq!(batches.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn handler_immutable_partial_selector_and_cancel_never_mutate() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("target.txt");
        std::fs::write(&file, "alpha\n").unwrap();
        let port = Port::new(file.clone(), false);
        let tool = port.tool(d.path(), EditMode::Replace);
        let native = EditHostPathPolicy {
            local_sandbox_root: Some(d.path().to_owned()),
            vault_roots: Some(vec![("_".into(), d.path().to_owned())]),
            ..Default::default()
        };
        for (url, scheme) in [("local://../x", "local"), ("vault://_/../x", "vault")] {
            let error = ara_tools::edit_urls::resolve_native_file(&native, url).unwrap_err().0;
            assert!(error.contains(&format!("{scheme}://")), "{error}");
            assert!(!error.contains("skill://"), "{error}");
        }
        for (policy, immutable, resource_immutable, path) in [
            (ContentUriEditPolicy::Handler, false, false, "local://target"),
            (ContentUriEditPolicy::File { plan_writable: true }, true, false, "local://target"),
            (ContentUriEditPolicy::ReadOnly, false, false, "local://target"),
            (ContentUriEditPolicy::File { plan_writable: true }, false, false, "local://target:1-2"),
            (ContentUriEditPolicy::File { plan_writable: true }, false, true, "local://target"),
        ] {
            {
                let mut state = port.state.lock().unwrap();
                state.policy = policy;
                state.immutable = immutable;
                state.resource_immutable = resource_immutable;
            }
            let result = run(&tool, replacement(path)).await;
            assert!(result.is_error, "{}", text(&result));
            assert_eq!(std::fs::read_to_string(&file).unwrap(), "alpha\n");
        }
        let delayed = Port::new(file.clone(), true);
        let tool = Arc::new(delayed.tool(d.path(), EditMode::Replace));
        let token = CancellationToken::new();
        let work_token = token.clone();
        let work = tool.clone();
        let task = tokio::spawn(async move {
            work.execute("cancel", args(replacement("local://target")), work_token, Arc::new(|_| {})).await
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), delayed.entered.notified()).await.unwrap();
        token.cancel();
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), task).await.unwrap().unwrap();
        assert!(result.is_err() || result.as_ref().is_ok_and(|o| o.is_error));
        assert_eq!(std::fs::read_to_string(file).unwrap(), "alpha\n");
    }

    #[derive(Clone)]
    struct PlanHost;
    impl EditHostPort for PlanHost {
        fn path_policy(&self) -> EditHostPathPolicy {
            EditHostPathPolicy { plan_active: true, ..Default::default() }
        }
    }
    #[tokio::test]
    async fn host_plan_scope_and_move_destination_resolve_before_any_write() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("target.txt");
        std::fs::write(&file, "alpha\n").unwrap();
        let port = Port::new(file.clone(), false);
        let ctx = ToolContext::new(d.path())
            .with_edit(EditMode::Replace, true)
            .with_uri_port(port.clone())
            .with_edit_host(Arc::new(PlanHost));
        let tool = EditTool::new(ctx);
        port.state.lock().unwrap().policy = ContentUriEditPolicy::File { plan_writable: false };
        assert!(run(&tool, replacement("local://target")).await.is_error);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "alpha\n");
        port.state.lock().unwrap().policy = ContentUriEditPolicy::File { plan_writable: true };
        assert!(!run(&tool, replacement("local://target:raw")).await.is_error);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "beta\n");
        // A missing grant for the destination must fail staging before the
        // source is changed, even though its URL resolved successfully.
        let move_tool = port.tool(d.path(), EditMode::ApplyPatch);
        let result=run(&move_tool,json!({"input":"*** Begin Patch\n*** Update File: local://target\n*** Move to: local://destination\n@@\n-beta\n+moved\n*** End Patch\n"})).await;
        assert!(result.is_error);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "beta\n");
        assert!(!d.path().join("destination").exists());
        let urls = port.urls.lock().unwrap();
        assert!(urls.iter().any(|u| u == "local://destination"));
    }
}
