//! Original fixed AST IDs + source contracts exercised as coarse module groups.
//! SOURCE candidate: tests must run on the delivered composition before acceptance.

use super::*;
use crate::{resolve::resolution_device, write::WriteTool};
use ara_ai::UserBlock;
fn output_text(output: &ToolOutput) -> String {
    output
        .content
        .iter()
        .filter_map(|c| if let UserBlock::Text(t) = c { Some(t.text.clone()) } else { None })
        .collect::<Vec<_>>()
        .join("\n")
}
fn input(pat: &str, out: &str, paths: &[&str]) -> AstEditOsValues {
    AstEditOsValues { ops: vec![(pat.into(), out.into())], paths: paths.iter().map(|s| s.to_string()).collect() }
}
async fn preview(ctx: &ToolContext, pat: &str, out: &str, paths: &[&str]) -> ToolOutput {
    AstEditTool { ctx: ctx.clone() }.execute_os_values(input(pat, out, paths), CancellationToken::new()).await.unwrap()
}
async fn resolve(ctx: &ToolContext, device: &str) -> Result<ToolOutput, ToolError> {
    WriteTool::new(ctx.clone()).execute_os_values(device, " reviewed codemod ", CancellationToken::new()).await
}
fn source_details(output: &ToolOutput) -> &Value {
    &output.details.as_ref().unwrap()["xdev"]["inner"]["sourceResultDetails"]
}

#[test]
fn ast_edit_schema_strict_and_source_validation_group() {
    // B-16d7d89232, B-1c92782a99. Exact required leaf structure is already
    // strict-representable; feed it through the existing validator too.
    let ctx = ToolContext::new(".");
    let definition = AstEditTool { ctx }.definition();
    assert_eq!(definition.strict, Some(true));
    assert_eq!(definition.parameters["required"], json!(["ops", "paths"]));
    assert_eq!(definition.parameters["properties"]["ops"]["items"]["required"], json!(["pat", "out"]));
    assert!(definition.parameters["properties"].get("preview").is_none());
    let args = json!({"ops":[{"pat":"foo($A)","out":"bar($A)"}],"paths":["x.ts"]}).as_object().unwrap().clone();
    assert_eq!(ara_ai::validation::validate_tool_arguments(&definition, "ast_edit", &args).unwrap(), args);
    assert_eq!(validate_ops(&[]).unwrap_err().0, "`ops` must include at least one op entry");
    assert!(validate_ops(&[("".into(), "x".into())]).unwrap_err().0.contains("ops[0].pat"));
    assert_eq!(
        validate_ops(&[("foo()".into(), "x".into()), ("foo()".into(), "y".into())]).unwrap_err().0,
        "Duplicate rewrite pattern: foo()"
    );
    for (raw, want) in [
        (None, 1000),
        (Some("1005 ignored"), 1005),
        (Some("0"), 1000),
        (Some("-1"), 1000),
        (Some("  +2.9"), 2),
        (Some("n/a"), 1000),
        (Some("4294967296"), 1),
        (Some("4294967298"), 2),
    ] {
        assert_eq!(max_files_from_env(raw), want);
    }
    assert_eq!(resolution_device(" XD://resolve "), Some("resolve"));
    for p in ["xd://Resolve", "xd://resolve/", "xd://resolve?x", "xd://reject#x"] {
        assert_eq!(resolution_device(p), None);
    }
}

#[tokio::test]
async fn ast_edit_original_hash_preview_apply_discard_and_tlaplus_group() {
    // B-cf9ac7a2c0, B-0e7c1c3b3c, B-55d727bf3f.
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path());
    let file = dir.path().join("calls.ts");
    std::fs::write(&file, "foo(1);\nfoo(2);\n").unwrap();
    let staged = preview(&ctx, "foo($A)", "bar($A)", &["calls.ts"]).await;
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "foo(1);\nfoo(2);\n");
    let text = output_text(&staged);
    assert!(text.starts_with(PREVIEW_PENDING_NOTICE));
    assert!(text.contains("[calls.ts#"));
    assert!(text.contains("-1:foo(1)"));
    assert!(text.contains("+1:bar(1)"));
    assert_eq!(staged.details.as_ref().unwrap()["totalReplacements"], 2);
    assert!(!staged.details.as_ref().unwrap()["displayContent"].as_str().unwrap().contains(PREVIEW_PENDING_NOTICE));
    let applied = resolve(&ctx, "xd://resolve").await.unwrap();
    assert_eq!(source_details(&applied)["totalReplacements"], 2);
    assert!(source_details(&applied)["applied"].as_bool().unwrap());
    assert!(output_text(&applied).contains("[calls.ts#"));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "bar(1);\nbar(2);\n");
    assert!(ctx.pending_resolution.pending_head().is_none());
    assert!(resolve(&ctx, "xd://resolve").await.is_err());
    preview(&ctx, "bar($A)", "discarded($A)", &["calls.ts"]).await;
    let discarded = resolve(&ctx, "xd://reject").await.unwrap();
    assert!(output_text(&discarded).starts_with("Discarded: AST Edit:"));
    assert!(!std::fs::read_to_string(&file).unwrap().contains("discarded"));
    assert_eq!(
        output_text(&resolve(&ctx, "xd://reject").await.unwrap()),
        "Nothing to reject; no pending action remains."
    );
    std::fs::write(
        dir.path().join("Spec.tla"),
        "---- MODULE Spec ----\nVARIABLE x\n\nInit == x = 0\n\nNext == x' = x + 1\n====\n",
    )
    .unwrap();
    // The fixed target moved TLA+ from a linked parser to the pinned wasm
    // grammar. The batch runner supplies this immutable real release asset.
    // Absence or a mismatched asset is a failed prerequisite, never a skip.
    let grammar_path = PathBuf::from(std::env::var_os("ARA_TEST_TLAPLUS_GRAMMAR")
        .expect("ARA_TEST_TLAPLUS_GRAMMAR must locate the pinned real TLA+ wasm"));
    let grammar = &ara_ast::language::wasm_grammars::TLAPLUS;
    assert_eq!(grammar_path.file_name().unwrap(), std::ffi::OsStr::new(grammar.file));
    let bytes = std::fs::read(&grammar_path).expect("read pinned real TLA+ grammar fixture");
    assert_eq!(bytes.len() as u64, grammar.size);
    use sha2::Digest as _;
    let digest = format!("{:x}", sha2::Sha256::digest(&bytes));
    assert_eq!(digest, grammar.sha256, "real TLA+ grammar SHA-256 pin");
    ara_ast::language::grammar::set_grammar_dir(grammar_path.parent().unwrap().to_path_buf());
    let tla = preview(&ctx, "Init", "Start", &["Spec.tla"]).await;
    assert_eq!(tla.details.as_ref().unwrap()["totalReplacements"], 1);
    assert!(tla.details.as_ref().unwrap().get("parseErrors").is_none());
    resolve(&ctx, "xd://resolve").await.unwrap();
    assert!(std::fs::read_to_string(dir.path().join("Spec.tla")).unwrap().contains("Start == x = 0"));
    let unicode = format!("foo(\"{}😀\");\n", "a".repeat(114));
    std::fs::write(dir.path().join("unicode.ts"), &unicode).unwrap();
    let failed = AstEditTool { ctx: ctx.clone() }
        .execute_os_values(input("foo($A)", "bar($A)", &["unicode.ts"]), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(failed.0.contains("unpaired UTF-16"));
    assert!(ctx.pending_resolution.pending_head().is_none());
    assert!(resolve(&ctx, "xd://resolve").await.is_err());
    assert_eq!(std::fs::read_to_string(dir.path().join("unicode.ts")).unwrap(), unicode);
}

#[tokio::test]
async fn ast_edit_original_stale_counts_and_failed_compute_owner_group() {
    // B-d997d53f6a plus the source's stale-less/more/apply-error branches.
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path());
    let file = dir.path().join("calls.ts");
    for (initial, current, want, fragment) in [
        ("foo();\n", "other();\n", 0, "no replacements were applied"),
        ("foo();\nfoo();\n", "foo();\n", 1, "only 1 of 2"),
        ("foo();\n", "foo();\nfoo();\n", 2, "applied 2 replacements but preview expected 1"),
    ] {
        std::fs::write(&file, initial).unwrap();
        preview(&ctx, "foo()", "bar()", &["calls.ts"]).await;
        std::fs::write(&file, current).unwrap();
        let applied = resolve(&ctx, "xd://resolve").await.unwrap();
        assert!(applied.is_error);
        assert!(output_text(&applied).contains(fragment));
        assert_eq!(source_details(&applied)["totalReplacements"], want);
        assert!(ctx.pending_resolution.pending_head().is_none());
        assert_eq!(std::fs::read_to_string(&file).unwrap().matches("bar()").count(), want as usize);
    }
    std::fs::write(&file, "foo(1);\n").unwrap();
    let tool = AstEditTool { ctx: ctx.clone() };
    tool.execute_os_values(
        AstEditOsValues {
            ops: vec![("foo($A)".into(), "bar($A)".into()), ("foo(1)".into(), "baz(1)".into())],
            paths: vec!["calls.ts".into()],
        },
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let head = ctx.pending_resolution.pending_head().unwrap();
    assert!(resolve(&ctx, "xd://resolve").await.unwrap_err().0.contains("Overlapping replacements"));
    assert_eq!(ctx.pending_resolution.pending_head().unwrap().0, head.0);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "foo(1);\n");
    resolve(&ctx, "xd://reject").await.unwrap();
    assert!(ctx.pending_resolution.pending_head().is_none());
}

#[tokio::test]
async fn ast_edit_original_nested_glob_scope_cap_error_and_render_group() {
    // B-f32e80f947 plus non-test source contracts.
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path());
    let root = dir.path().join("packages/pkg-a/src");
    std::fs::create_dir_all(root.join("nested")).unwrap();
    for path in
        [root.join("root.ts"), root.join("nested/child.ts"), root.join("ignored.js"), dir.path().join("outside.ts")]
    {
        std::fs::write(path, "foo(1);\n").unwrap();
    }
    let staged = preview(&ctx, "foo($A)", "bar($A)", &["packages/pkg-*/src/**/*.ts"]).await;
    assert_eq!(staged.details.as_ref().unwrap()["totalReplacements"], 2);
    let text = output_text(&staged);
    assert!(text.contains("## root.ts#"));
    assert!(text.contains("### child.ts#"));
    assert!(!text.contains("ignored.js"));
    resolve(&ctx, "xd://resolve").await.unwrap();
    assert_eq!(std::fs::read_to_string(root.join("ignored.js")).unwrap(), "foo(1);\n");
    let scope = resolve_scope(&ctx, &["packages/pkg-*/src/**/*.ts".into()], CancellationToken::new()).await.unwrap();
    let capped =
        ast_edit_engine::run(&scope, &[("bar($A)".into(), "baz($A)".into())], true, 1, CancellationToken::new())
            .unwrap();
    assert!(capped.limit_reached);
    assert_eq!(capped.files_touched, 1);
    let (errors, total) = ast_edit_engine::cap_errors(
        (0..31).map(|n| format!("parse {n}")).chain(std::iter::once("parse 0".into())).collect(),
    );
    assert_eq!(errors.len(), 20);
    assert_eq!(total, 31);
    for path in ["https://example.com/a.ts", "www.example.com/a.ts"] {
        assert!(
            resolve_scope(&ctx, &[path.into()], CancellationToken::new())
                .await
                .unwrap_err()
                .0
                .contains("Cannot rewrite external URL")
        );
    }
    let render = crate::ast_edit_render::result(&staged, &[("  foo(\n\t$A)  ".into(), "bar($A)".into())], true);
    assert!(render.proposed_badge);
    assert_eq!(render.frame_state, crate::ast_edit_render::AstRenderState::Pending);
    assert_eq!(render.description, Some("foo( $A)".into()));
    assert!(!render.groups.is_empty());
    assert!(render.groups.iter().flatten().any(|line| line.hyperlink.as_ref().is_some_and(|p| p.ends_with("root.ts"))));
    let collapsed = render.body(false);
    assert!(!collapsed.is_empty());
    assert!(!collapsed.iter().any(|l| l.text.contains(PREVIEW_PENDING_NOTICE)));
}

#[tokio::test]
async fn ast_edit_internal_backing_local_and_queue_head_group() {
    use crate::edit_host::{EditHostPathPolicy, EditHostPort};
    struct Host(PathBuf);
    impl EditHostPort for Host {
        fn path_policy(&self) -> EditHostPathPolicy {
            EditHostPathPolicy { local_sandbox_root: Some(self.0.clone()), ..Default::default() }
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let local = dir.path().join("local");
    std::fs::create_dir(&local).unwrap();
    std::fs::write(local.join("a.ts"), "foo();\n").unwrap();
    let skill = dir.path().join("skill");
    std::fs::create_dir(&skill).unwrap();
    std::fs::write(skill.join("b.ts"), "foo();\n").unwrap();
    let ctx = ToolContext::new(dir.path()).with_edit_host(Arc::new(Host(local.clone()))).with_skills(vec![
        crate::internal_urls::SkillRef {
            name: "demo".into(),
            file_path: skill.join("SKILL.md"),
            base_dir: skill.clone(),
        },
    ]);
    preview(&ctx, "foo()", "localDone()", &["local://a.ts"]).await;
    let first = ctx.pending_resolution.pending_head().unwrap();
    preview(&ctx, "foo()", "skillDone()", &["skill://demo/b.ts"]).await;
    let second = ctx.pending_resolution.pending_head().unwrap();
    assert_ne!(first.0, second.0);
    resolve(&ctx, "xd://resolve").await.unwrap();
    assert_eq!(ctx.pending_resolution.pending_head().unwrap().0, first.0);
    resolve(&ctx, "xd://reject").await.unwrap();
    assert_eq!(std::fs::read_to_string(local.join("a.ts")).unwrap(), "foo();\n");
    assert!(std::fs::read_to_string(skill.join("b.ts")).unwrap().contains("skillDone"));
    assert!(resolve_scope(&ctx, &["local://../x.ts".into()], CancellationToken::new()).await.is_err());
    assert!(
        resolve_scope(&ctx, &["skill://demo/*.ts".into()], CancellationToken::new())
            .await
            .unwrap_err()
            .0
            .contains("Glob patterns are not supported")
    );
    struct InFlight;
    #[async_trait]
    impl crate::resolve::QueueResolutionInvoker for InFlight {
        async fn invoke(&self, action: &str, reason: &str) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::text("in-flight directive owns resolution")
                .with_details(json!({"action":action,"reason":reason,"label":"existing queue"})))
        }
    }
    preview(&ctx, "foo()", "deferred()", &["local://a.ts"]).await;
    let head = ctx.pending_resolution.pending_head().unwrap();
    ctx.pending_resolution.set_queue_invoker(Some(Arc::new(InFlight)));
    let queued = resolve(&ctx, "xd://resolve").await.unwrap();
    assert!(output_text(&queued).contains("in-flight directive"));
    assert_eq!(ctx.pending_resolution.pending_head().unwrap().0, head.0);
    assert_eq!(std::fs::read_to_string(local.join("a.ts")).unwrap(), "foo();\n");
    ctx.pending_resolution.set_queue_invoker(None);
    resolve(&ctx, "xd://reject").await.unwrap();
    struct AuthoredToolError;
    #[async_trait]
    impl crate::resolve::PendingResolution for AuthoredToolError {
        async fn apply(&self, _: &str) -> Result<ToolOutput, crate::resolve::PendingApplyFault> {
            Err(ToolError("authored ToolError remains exact".into()).into())
        }
    }
    let id = ctx.pending_resolution.register("fixture", "source class".into(), Arc::new(AuthoredToolError));
    assert_eq!(resolve(&ctx, "xd://resolve").await.unwrap_err().0, "authored ToolError remains exact");
    assert_eq!(ctx.pending_resolution.pending_head().unwrap().0, id);
    resolve(&ctx, "xd://reject").await.unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(AstEditTool { ctx }.execute_os_values(input("foo()", "bar()", &["outside.ts"]), cancel).await.is_err());
}
