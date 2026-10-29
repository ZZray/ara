//! Fixed OMP 596f2da READ original bodies; original IDs stay in the source map.
//! MIT / Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
use ara_agent::{AgentTool, ToolOutput, UpdateFn};
use ara_ai::{JsonObject, UserBlock};
use ara_tools::{ToolContext, edit::EditTool, read::ReadTool};
use base64::Engine;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

fn args(value: Value) -> JsonObject {
    value.as_object().unwrap().clone()
}
fn noop() -> UpdateFn {
    Arc::new(|_| {})
}
fn text(output: &ToolOutput) -> String {
    output
        .content
        .iter()
        .filter_map(|block| match block {
            UserBlock::Text(block) => Some(block.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
async fn read(ctx: &ToolContext, path: impl AsRef<str>) -> ToolOutput {
    ReadTool { ctx: ctx.clone() }
        .execute(
            "read",
            args(json!({"path":path.as_ref()})),
            CancellationToken::new(),
            noop(),
        )
        .await
        .unwrap()
}
async fn edit(ctx: &ToolContext, input: String) -> ToolOutput {
    EditTool::new(ctx.clone())
        .execute("edit", args(json!({"input":input})), CancellationToken::new(), noop())
        .await
        .unwrap()
}
fn numbered(lines: usize) -> String {
    (1..=lines)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}
fn header(text: &str) -> &str {
    text.lines().next().unwrap()
}
fn tag(header: &str) -> &str {
    &header[header.len() - 5..header.len() - 1]
}
fn matches(pattern: &str, text: &str) {
    assert!(regex::Regex::new(pattern).unwrap().is_match(text), "{text}");
}
struct Env {
    dir: tempfile::TempDir,
    ctx: ToolContext,
}
impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path());
        Self { dir, ctx }
    }
    fn write(&self, path: &str, content: impl AsRef<[u8]>) -> PathBuf {
        let file = self.dir.path().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, content).unwrap();
        file
    }
}

#[tokio::test]
async fn original_read_r0_multi_range_eight() {
    // B-19d6c3d8e4: Node path.join in the original POSIX reference fixture.
    // Production formatPathRelativeToCwd also explicitly normalizes to POSIX.
    {
        let env = Env::new();
        let file = env.write("src/nested/numbered.txt", "alpha\nbeta\n");
        let body = text(&read(&env.ctx, file.to_string_lossy()).await);
        let first = header(&body);
        assert_eq!(first, format!("[src/nested/numbered.txt#{}]", tag(first)));
    }
    // B-a171872c86
    {
        let env = Env::new();
        let file = env.write("src/numbered.txt", numbered(50));
        let body = text(&read(&env.ctx, format!("{}:3-5,20-22", file.display())).await);
        matches(r"^\[src/numbered\.txt#[0-9A-F]{4}\]$", header(&body));
        for line in [3, 4, 5, 20, 21, 22] {
            assert!(body.contains(&format!("line {line}")), "{body}");
        }
        for line in [10, 19] {
            assert!(!body.contains(&format!("line {line}")), "{body}");
        }
        assert!(body.contains('…'));
    }
    // B-c793cae5ee
    {
        let env = Env::new();
        let file = env.write("numbered.txt", numbered(20));
        let body = text(&read(&env.ctx, format!("{}:3-7,6-9", file.display())).await);
        for line in [3, 4, 5, 6, 7, 8, 9] {
            assert!(body.contains(&format!("line {line}\n")), "{body}");
        }
        assert!(!body.contains('…'), "{body}");
    }
    // B-e91ebe2e9e
    {
        let env = Env::new();
        let file = env.write("numbered.txt", numbered(50));
        let body = text(&read(&env.ctx, format!("{}:30-32,5-7", file.display())).await);
        let early = body.find("line 5").unwrap();
        let late = body.find("line 30").unwrap();
        assert!(late > early);
    }
    // B-18df6de5ee
    {
        let env = Env::new();
        let file = env.write("small.txt", numbered(10));
        let body = text(&read(&env.ctx, format!("{}:3-5,999-1000", file.display())).await);
        assert!(body.contains("line 3") && body.contains("line 5"));
        assert!(
            body.contains("Range 999-1000 is beyond end of file (10 lines total); skipped"),
            "{body}"
        );
    }
    // B-44a75b5ea1
    {
        let env = Env::new();
        let file = env.write("numbered.txt", numbered(30));
        let body = text(&read(&env.ctx, format!("{}:2+2,20+2", file.display())).await);
        for line in [2, 3, 20, 21] {
            assert!(body.contains(&format!("line {line}")), "{body}");
        }
        for line in [4, 19] {
            assert!(!body.contains(&format!("line {line}")), "{body}");
        }
    }
    // B-98ec64cb85
    {
        let env = Env::new();
        let file = env.write("numbered.txt", numbered(30));
        let dotdot = text(&read(&env.ctx, format!("{}:3..5", file.display())).await);
        let dash = text(&read(&env.ctx, format!("{}:3-5", file.display())).await);
        assert!(dotdot.contains("line 3") && dotdot.contains("line 5"));
        assert_eq!(dotdot, dash);
    }
    // B-86b6fc49f2
    {
        let env = Env::new();
        let file = env.write("numbered.txt", numbered(50));
        let body = text(&read(&env.ctx, format!("{}:3..5,20..22", file.display())).await);
        for line in [3, 5, 20, 22] {
            assert!(body.contains(&format!("line {line}")), "{body}");
        }
        assert!(!body.contains("line 10") && body.contains('…'));
    }
}

#[tokio::test]
async fn original_read_r0_raw_range_four() {
    for (id, selector) in [
        ("B-2cd266f50f", "raw:31-31"),
        ("B-e6c2d72598", "raw:1-2"),
        ("B-7b3510c34a", "raw:31-"),
        ("B-75a01e4ceb", "31-31"),
    ] {
        let env = Env::new();
        let content = (1..=60).map(|n| format!("L{n:02}")).collect::<Vec<_>>().join("\n");
        let file = env.write("data.txt", content);
        let result = read(&env.ctx, format!("{}:{selector}", file.display())).await;
        let body = text(&result);
        match id {
            "B-2cd266f50f" => assert_eq!(body.trim_end(), "L31"),
            "B-e6c2d72598" => assert_eq!(body.trim_end(), "L01\nL02"),
            "B-7b3510c34a" => assert_eq!(result.details.as_ref().unwrap()["totalLines"], json!(60)),
            "B-75a01e4ceb" => assert!(
                body.contains("L31") && body.contains("L30") && body.contains("L32"),
                "{body}"
            ),
            _ => unreachable!(),
        }
    }
}

#[tokio::test]
async fn original_read_r0_single_pass_two() {
    // B-d0501bdf36
    {
        let env = Env::new();
        let file = env.write("bom.ts", "\u{FEFF}export const a = 1;\nexport const b = 2;\n");
        let body = text(&read(&env.ctx, file.to_string_lossy()).await);
        let first = header(&body);
        matches(r"^\[bom\.ts#[0-9A-F]{4}\]$", first);
        edit(&env.ctx, format!("{first}\nPUT 2.=2:\n+export const b = 22;")).await;
        assert_eq!(
            std::fs::read_to_string(file).unwrap(),
            "\u{FEFF}export const a = 1;\nexport const b = 22;\n"
        );
    }
    // B-eca9904eda
    {
        let env = Env::new();
        let mut bytes = vec![b'z'; 60 * 1024 - 512];
        bytes.extend(vec![0xff; 512]);
        bytes.extend(b"\ntail\n");
        let file = env.write("invalid-utf8.txt", bytes);
        let body = text(&read(&env.ctx, format!("{}:1-1", file.display())).await);
        assert!(body.contains("[Line 1 is 60.0KB, exceeds 50.0KB limit"), "{body}");
        assert!(!body.contains("61.0KB"), "{body}");
    }
}

async fn home_roundtrip() {
    // B-d52ce7e4d8: actual os.homedir in a child with an isolated home directory.
    let env = Env::new();
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let home_dir = tempfile::Builder::new()
        .prefix(".omp-read-edit-")
        .tempdir_in(&home)
        .unwrap();
    let file = home_dir.path().join("settings.txt");
    std::fs::write(&file, "alpha\nbeta\n").unwrap();
    let authored = format!("~/{}", file.strip_prefix(&home).unwrap().to_string_lossy());
    let body = text(&read(&env.ctx, authored).await);
    let first = header(&body);
    let result = edit(&env.ctx, format!("{first}\nPUT 1-1:\n+ALPHA\n")).await;
    assert!(!result.is_error, "{}", text(&result));
    let actual = std::fs::read_to_string(&file).unwrap();
    assert_eq!(actual, "ALPHA\nbeta\n");
    println!(
        "ARA_R0_HOME_CHILD_RESULT {}",
        json!({"original_id":"B-d52ce7e4d8","header":first,"actual":actual,"file":file})
    );
}

#[tokio::test]
async fn original_read_r0_path_roundtrips_three() {
    if std::env::var_os("ARA_R0_ISOLATED_HOME_CHILD").is_some() {
        home_roundtrip().await;
        return;
    }
    // B-fc10f5b96f
    {
        let env = Env::new();
        let out = tempfile::tempdir().unwrap();
        let file = out.path().join("settings.json");
        std::fs::write(&file, "alpha\nbeta\n").unwrap();
        let body = text(&read(&env.ctx, file.to_string_lossy()).await);
        let first = header(&body);
        matches(r"^\[.+settings\.json#[0-9A-F]{4}\]$", first);
        assert!(
            first.contains(out.path().file_name().unwrap().to_str().unwrap()),
            "{first}"
        );
        let result = edit(&env.ctx, format!("{first}\nPUT 1-1:\n+ALPHA\n")).await;
        assert!(!text(&result).contains("File not found"));
        assert_eq!(std::fs::read_to_string(file).unwrap(), "ALPHA\nbeta\n");
    }
    // B-d52ce7e4d8: same executable; no parent HOME/USERPROFILE mutation.
    {
        let home = tempfile::tempdir().unwrap();
        let stdout = tempfile::tempfile().unwrap();
        let stderr = tempfile::tempfile().unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "original_read_r0_path_roundtrips_three",
                "--test-threads=1",
                "--nocapture",
            ])
            .env("ARA_R0_ISOLATED_HOME_CHILD", "1")
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .stdout(stdout.try_clone().unwrap())
            .stderr(stderr.try_clone().unwrap())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("isolated HOME child deadline");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        use std::io::{Read, Seek, SeekFrom};
        let mut stdout = stdout;
        let mut stderr = stderr;
        let mut a = String::new();
        let mut b = String::new();
        stdout.seek(SeekFrom::Start(0)).unwrap();
        stderr.seek(SeekFrom::Start(0)).unwrap();
        stdout.read_to_string(&mut a).unwrap();
        stderr.read_to_string(&mut b).unwrap();
        println!(
            "ARA_R0_HOME_CHILD_OUTPUT {}",
            json!({"stdout_base64":base64::engine::general_purpose::STANDARD.encode(a.as_bytes()),"stderr_base64":base64::engine::general_purpose::STANDARD.encode(b.as_bytes()),"exit_code":status.code()})
        );
        if !status.success() {
            panic!("isolated HOME child {status}: {a}\n{b}");
        }
    }
    // B-b32cbf3557: POSIX reference fixture path.join; real files stay native.
    {
        let env = Env::new();
        let root = env.write("settings.json", "root\n");
        let nested = env.write("src/settings.json", "alpha\nbeta\n");
        let body = text(&read(&env.ctx, nested.to_string_lossy()).await);
        let first = header(&body);
        assert_eq!(first, format!("[src/settings.json#{}]", tag(first)));
        edit(&env.ctx, format!("{first}\nPUT 1.=1:\n+ALPHA\n")).await;
        assert_eq!(std::fs::read_to_string(nested).unwrap(), "ALPHA\nbeta\n");
        assert_eq!(std::fs::read_to_string(root).unwrap(), "root\n");
    }
}

#[tokio::test]
async fn original_read_r0_blank_line_one() {
    // B-94e67a6bc4: fixture lines are below the original 64-column cap.
    // The separate four cap originals still require the production cap port.
    let env = Env::new();
    let file = env.write("eof-blank.txt", "first\n\nlast\n");
    let body = text(&read(&env.ctx, file.to_string_lossy()).await);
    assert!(body.contains("1:first\n2:\n3:last"), "{body}");
    assert!(!body.contains("\n4:"), "{body}");
    let first = header(&body);
    matches(r"^\[([^#\r\n]+)#[0-9A-F]{4}\]$", first);
    edit(&env.ctx, format!("{first}\nCUT 2\n")).await;
    assert_eq!(std::fs::read_to_string(file).unwrap(), "first\nlast\n");
}

#[tokio::test]
async fn original_read_workspace_suffix_and_terminal_newline_group() {
    // B-b6b787f84c: fixed read-edit-out-of-cwd.test.ts:118, all three modes.
    let dir = tempfile::tempdir().unwrap();
    for (mode, name) in [
        (ara_edit::EditMode::Replace, "replace"),
        (ara_edit::EditMode::Patch, "patch"),
        (ara_edit::EditMode::ApplyPatch, "apply_patch"),
    ] {
        let file_name = format!("{name}.txt");
        let workspace_file = dir.path().join("src").join(&file_name);
        std::fs::create_dir_all(workspace_file.parent().unwrap()).unwrap();
        std::fs::write(&workspace_file, "alpha\nbeta\n").unwrap();
        let ctx = ToolContext::new(dir.path()).with_edit(mode, true);
        assert!(text(&read(&ctx, &file_name).await).contains("alpha"));
        let input = match mode {
            ara_edit::EditMode::Replace => {
                json!({"path":file_name,"old_string":"alpha","new_string":"ALPHA"})
            }
            ara_edit::EditMode::Patch => {
                json!({"path":file_name,"edits":[{"op":"update","diff":"@@\n-alpha\n+ALPHA"}]})
            }
            ara_edit::EditMode::ApplyPatch => json!({"input":format!(
                "*** Begin Patch\n*** Update File: {file_name}\n@@\n-alpha\n+ALPHA\n*** End Patch\n"
            )}),
            _ => unreachable!(),
        };
        EditTool::new(ctx)
            .execute(
                "edit-workspace-suffix",
                args(input),
                CancellationToken::new(),
                noop(),
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&workspace_file).unwrap(),
            "ALPHA\nbeta\n"
        );
    }

    // B-e2f9153f60: fixed same-authored-path Delete/Add keeps src/recreate.txt.
    {
        let env = Env::new();
        let file = env.write("src/recreate.txt", "alpha\nbeta\n");
        let ctx = env.ctx.with_edit(ara_edit::EditMode::ApplyPatch, true);
        assert!(text(&read(&ctx, "recreate.txt").await).contains("alpha"));
        edit(&ctx, "*** Begin Patch\n*** Delete File: recreate.txt\n*** Add File: recreate.txt\n+rewritten\n*** End Patch\n".into()).await;
        assert_eq!(std::fs::read_to_string(file).unwrap(), "rewritten\n");
        assert!(!env.dir.path().join("recreate.txt").exists());
    }

    // B-d0110cf0c4: fixed read-single-pass.test.ts:100, all three assertions.
    {
        let env = Env::new();
        let file = env.write("trailing.txt", "alpha\nbeta\n");
        assert_eq!(
            text(&read(&env.ctx, format!("{}:3", file.display())).await),
            "Line 3 is beyond end of file (2 lines total). Use :1 to read from the start, or :2 to read the last line."
        );
        assert_eq!(
            text(&read(&env.ctx, format!("{}:raw:3", file.display())).await),
            ""
        );
        assert!(
            text(&read(&env.ctx, format!("{}:raw:4", file.display())).await)
                .contains("beyond end of file (3 lines total)")
        );
    }
}


#[tokio::test]
async fn read_original_plan_reference_alias_group() {
    // Source branch read.ts744-760,1402-1447; no fabricated canonical B-ID.
    struct Reference(std::sync::RwLock<Option<PathBuf>>);
    impl ara_tools::read::ReadPlanReferencePort for Reference {
        fn read_reference_path(&self) -> Option<PathBuf> { self.0.read().unwrap().clone() }
    }
    async fn missing(ctx: &ToolContext, path: &str) {
        let result = ReadTool { ctx: ctx.clone() }.execute("missing-plan", args(json!({"path":path})),
            CancellationToken::new(), noop()).await;
        assert!(result.is_err(), "missing alias must stay a READ failure: {result:?}");
    }
    let work = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    let target = local.path().join("PLAN.md");
    std::fs::write(&target, "plan first\nplan second\nplan third\n").unwrap();
    let reference = Arc::new(Reference(std::sync::RwLock::new(Some(target.clone()))));
    let ctx = ToolContext::new(work.path()).with_read_plan_reference_port(reference.clone());
    let alias = read(&ctx, "PLAN.md").await;
    assert!(text(&alias).contains("plan first"));
    let details = alias.details.as_ref().unwrap();
    assert_eq!(details["resolvedPath"], json!(target.to_string_lossy()));
    assert!(details["suffixResolution"].is_null());
    assert_ne!(details["meta"]["source"]["type"], "internal", "{details}");
    assert!(ctx.edit_store.head(&ara_edit::path_policy::canonical_key(&target)).is_some(), "alias uses mutable file snapshots");
    assert_eq!(text(&read(&ctx, "PLAN.md:raw:2-2").await), "plan second");
    assert_eq!(text(&read(&ctx, format!("{}:raw:1-1", work.path().join("PLAN.md").display())).await), "plan first");
    let ranges = read(&ctx, "PLAN.md:raw:1-1,3-3").await;
    assert!(text(&ranges).contains("plan first") && text(&ranges).contains("plan third"));
    missing(&ctx, "nested/PLAN.md").await;
    missing(&ctx, "OTHER.md").await;
    std::fs::write(work.path().join("PLAN.md"), "real cwd\n").unwrap();
    assert_eq!(text(&read(&ctx, "PLAN.md:raw").await), "real cwd\n");
    std::fs::remove_file(work.path().join("PLAN.md")).unwrap();
    std::fs::create_dir(work.path().join("source")).unwrap();
    std::fs::write(work.path().join("source/PLAN.md"), "unique suffix\n").unwrap();
    let suffix = read(&ctx, "PLAN.md:raw").await;
    assert!(text(&suffix).contains("unique suffix"));
    assert_eq!(suffix.details.as_ref().unwrap()["suffixResolution"]["to"], "source/PLAN.md");
    std::fs::remove_file(work.path().join("source/PLAN.md")).unwrap();
    std::fs::write(work.path().join("left.txt"), "left scope\n").unwrap();
    std::fs::write(work.path().join("right.txt"), "right scope\n").unwrap();
    let listed_target = local.path().join("left.txt;right.txt");
    std::fs::write(&listed_target, "alias must lose to explicit scope\n").unwrap();
    *reference.0.write().unwrap() = Some(listed_target);
    let listed = text(&read(&ctx, "left.txt;right.txt").await);
    assert!(listed.contains("left scope") && listed.contains("right scope") && !listed.contains("alias must lose"), "{listed}");
    *reference.0.write().unwrap() = None;
    missing(&ctx, "PLAN.md").await;
    *reference.0.write().unwrap() = Some(target.clone());
    std::fs::remove_file(&target).unwrap();
    missing(&ctx, "PLAN.md").await;
    std::fs::write(&target, "reference changed\n").unwrap();
    assert_eq!(text(&read(&ctx, "PLAN.md:raw").await), "reference changed\n");
    missing(&ToolContext::new(work.path()), "PLAN.md").await;
}

// b91db66e read1140-1158,1887-1940: issuing-reference/locator and all saved tails.
#[tokio::test]
async fn read_plan_uri_reference_continuation_group() {
    use ara_agent::ToolError;
    use ara_tools::{ContentUriPort, ContentUriRoute, UriResource};
    use ara_tools::read::{ReadPlanReferencePort, ReadPlanReferenceSnapshot};
    use std::sync::{Mutex, atomic::{AtomicBool,AtomicUsize,Ordering}};
    struct Reference { uri:Mutex<Option<String>>, calls:AtomicUsize, error:AtomicBool }
    impl ReadPlanReferencePort for Reference {
        fn read_reference_path(&self)->Option<PathBuf> { panic!("URI snapshot must not re-read legacy state"); }
        fn read_reference_snapshot(&self)->Result<ReadPlanReferenceSnapshot,ToolError> {
            self.calls.fetch_add(1,Ordering::SeqCst);
            if self.error.load(Ordering::SeqCst) { return Err(ToolError("snapshot failure".into())); }
            Ok(self.uri.lock().unwrap().clone().map_or(ReadPlanReferenceSnapshot::Absent,|authored|
                ReadPlanReferenceSnapshot::Uri {authored,native_local_fallback:None}))
        }
    }
    struct Locator {
        paths:Mutex<std::collections::HashMap<String,PathBuf>>, calls:Mutex<Vec<String>>,
        route:AtomicUsize, failure:AtomicBool, cancel:AtomicBool, stale:AtomicBool,
        change:Mutex<Option<Arc<Reference>>>,
    }
    #[async_trait::async_trait]
    impl ContentUriPort for Locator {
        fn route(&self,url:&str)->ContentUriRoute {
            // Make an empty-reference regression distinguishable: v1 would locate.
            if url.is_empty() {return ContentUriRoute::Registered{writable:false};}
            if !url.starts_with("plan://") {return ContentUriRoute::Unregistered;}
            match self.route.load(Ordering::SeqCst) {1=>ContentUriRoute::Removed,2=>ContentUriRoute::Unregistered,_=>ContentUriRoute::Registered{writable:false}}
        }
        async fn search_path_only(&self,url:&str,cancel:CancellationToken)->Result<Option<PathBuf>,ToolError> {
            self.calls.lock().unwrap().push(url.into());
            if let Some(reference)=self.change.lock().unwrap().take() { *reference.uri.lock().unwrap()=Some("plan://next.md".into()); }
            if self.failure.load(Ordering::SeqCst) {return Err(ToolError("locator failure".into()));}
            if self.cancel.load(Ordering::SeqCst) {cancel.cancel();return Err(ToolError("locator cancelled".into()));}
            let path=self.paths.lock().unwrap().get(url).cloned();
            if self.stale.load(Ordering::SeqCst) && let Some(path)=&path {std::fs::remove_file(path).unwrap();}
            Ok(path)
        }
        async fn read(&self,_:&str,_:CancellationToken)->Result<UriResource,ToolError> {panic!("alias must never materialize a virtual resource")}
        async fn write(&self,_:&str,_:&str,_:CancellationToken)->Result<(),ToolError> {panic!("alias must not write")}
    }
    async fn execute(ctx:&ToolContext,path:&str,cancel:CancellationToken)->Result<ToolOutput,ToolError> {
        tokio::time::timeout(Duration::from_secs(5),ReadTool{ctx:ctx.clone()}.execute(
            "uri-alias",args(json!({"path":path})),cancel,noop())).await.expect("bounded READ fixture")
    }
    let work=tempfile::tempdir().unwrap();let local=tempfile::tempdir().unwrap();
    let target=local.path().join("PLAN.md");std::fs::write(&target,"first\nsecond\nthird\n").unwrap();
    let reference=Arc::new(Reference{uri:Mutex::new(Some("plan://current".into())),calls:AtomicUsize::new(0),error:AtomicBool::new(false)});
    let locator=Arc::new(Locator {paths:Mutex::new(std::collections::HashMap::from([("plan://current".into(),target.clone())])),
        calls:Mutex::new(vec![]),route:AtomicUsize::new(0),failure:AtomicBool::new(false),cancel:AtomicBool::new(false),stale:AtomicBool::new(false),change:Mutex::new(None)});
    let ctx=ToolContext::new(work.path()).with_read_plan_reference_port(reference.clone()).with_uri_port(locator.clone());
    let reset=|| {reference.calls.store(0,Ordering::SeqCst);locator.calls.lock().unwrap().clear();};
    let counts=|get,locate| {assert_eq!(reference.calls.load(Ordering::SeqCst),get);assert_eq!(locator.calls.lock().unwrap().len(),locate);};
    for (path,expected) in [("PLAN.md:raw:2-2","second"),("PLAN.md:raw:1-1,3-3","first")] {
        reset();let output=execute(&ctx,path,CancellationToken::new()).await.unwrap();
        assert!(text(&output).contains(expected));assert_eq!(output.details.as_ref().unwrap()["resolvedPath"],json!(target.to_string_lossy()));
        assert!(output.details.as_ref().unwrap()["suffixResolution"].is_null());counts(1,1);
    }
    eprintln!("ARA-PLAN-URI PASS plain-selectors");
    reset();execute(&ctx,"nested/PLAN.md",CancellationToken::new()).await.unwrap_err();counts(1,0);
    reset();execute(&ctx,"OTHER.md",CancellationToken::new()).await.unwrap_err();counts(1,1);
    reference.error.store(true,Ordering::SeqCst);
    for path in ["PLAN.md","nested/PLAN.md"] {reset();assert_eq!(execute(&ctx,path,CancellationToken::new()).await.unwrap_err().0,"snapshot failure");counts(1,0);}
    reference.error.store(false,Ordering::SeqCst);
    eprintln!("ARA-PLAN-URI PASS getter-before-parent-error-and-basename");
    // Precedence must avoid both snapshot and locator, rather than locating early.
    std::fs::write(work.path().join("PLAN.md"),"cwd literal\n").unwrap();reset();
    assert_eq!(text(&execute(&ctx,"PLAN.md:raw",CancellationToken::new()).await.unwrap()),"cwd literal\n");counts(0,0);
    std::fs::remove_file(work.path().join("PLAN.md")).unwrap();
    std::fs::create_dir(work.path().join("source")).unwrap();std::fs::write(work.path().join("source/PLAN.md"),"suffix\n").unwrap();reset();
    let suffix=execute(&ctx,"PLAN.md:raw",CancellationToken::new()).await.unwrap();assert!(text(&suffix).contains("suffix"));counts(0,0);
    std::fs::remove_file(work.path().join("source/PLAN.md")).unwrap();
    std::fs::write(work.path().join("left.txt"),"left\n").unwrap();std::fs::write(work.path().join("right.txt"),"right\n").unwrap();reset();
    let list=execute(&ctx,"left.txt;right.txt",CancellationToken::new()).await.unwrap();assert!(text(&list).contains("left")&&text(&list).contains("right"));counts(0,0);
    // Existing SQLite READ selection remains ahead of plan resolution.
    let db=work.path().join("data.db");let connection=rusqlite::Connection::open(&db).unwrap();
    connection.execute_batch("CREATE TABLE entries(value TEXT); INSERT INTO entries VALUES ('database');").unwrap();drop(connection);reset();
    let output=execute(&ctx,"data.db:entries",CancellationToken::new()).await.unwrap();assert!(text(&output).contains("value"));counts(0,0);
    std::fs::write(work.path().join("existing.tar.gz"),include_bytes!("../src/write_archive/read_facade/fixtures/ar.tar.gz")).unwrap();reset();
    let archive_output=execute(&ctx,"existing.tar.gz",CancellationToken::new()).await.unwrap();assert!(!text(&archive_output).is_empty());counts(0,0);
    eprintln!("ARA-PLAN-URI PASS literal-suffix-semicolon-sqlite-archive-priority");
    for mode in [1,2] {locator.route.store(mode,Ordering::SeqCst);reset();execute(&ctx,"PLAN.md",CancellationToken::new()).await.unwrap_err();counts(1,0);}
    locator.route.store(0,Ordering::SeqCst);
    locator.failure.store(true,Ordering::SeqCst);reset();let err=execute(&ctx,"PLAN.md",CancellationToken::new()).await.unwrap_err();assert_ne!(err.0,"locator failure");counts(1,1);locator.failure.store(false,Ordering::SeqCst);
    locator.paths.lock().unwrap().remove("plan://current");reset();execute(&ctx,"PLAN.md",CancellationToken::new()).await.unwrap_err();counts(1,1);
    locator.paths.lock().unwrap().insert("plan://current".into(),target.clone());
    locator.stale.store(true,Ordering::SeqCst);reset();execute(&ctx,"PLAN.md",CancellationToken::new()).await.unwrap_err();counts(1,1);locator.stale.store(false,Ordering::SeqCst);
    std::fs::write(&target,"recreated\n").unwrap();reset();assert!(text(&execute(&ctx,"PLAN.md",CancellationToken::new()).await.unwrap()).contains("recreated"));counts(1,1);
    locator.cancel.store(true,Ordering::SeqCst);reset();let token=CancellationToken::new();assert!(execute(&ctx,"PLAN.md",token.clone()).await.is_err());assert!(token.is_cancelled());counts(1,1);locator.cancel.store(false,Ordering::SeqCst);
    *reference.uri.lock().unwrap()=Some(String::new());reset();execute(&ctx,"PLAN.md",CancellationToken::new()).await.unwrap_err();counts(1,0);
    *reference.uri.lock().unwrap()=None;reset();execute(&ctx,"PLAN.md",CancellationToken::new()).await.unwrap_err();counts(1,0);*reference.uri.lock().unwrap()=Some("plan://current".into());
    eprintln!("ARA-PLAN-URI PASS missing-removed-error-stale-cancel-absence");
    // A concurrent reference rewrite changes the next request, never the issuing locator.
    *locator.change.lock().unwrap()=Some(reference.clone());reset();assert!(text(&execute(&ctx,"PLAN.md",CancellationToken::new()).await.unwrap()).contains("recreated"));counts(1,1);
    assert_eq!(locator.calls.lock().unwrap()[0],"plan://current");assert_eq!(reference.uri.lock().unwrap().as_deref(),Some("plan://next.md"));
    *reference.uri.lock().unwrap()=Some("plan://current".into());
    let another=tempfile::tempdir().unwrap();let other_target=another.path().join("PLAN.md");std::fs::write(&other_target,"other session\n").unwrap();
    let other_port=Arc::new(Locator{paths:Mutex::new(std::collections::HashMap::from([("plan://current".into(),other_target.clone())])),calls:Mutex::new(vec![]),
        route:AtomicUsize::new(0),failure:AtomicBool::new(false),cancel:AtomicBool::new(false),stale:AtomicBool::new(false),change:Mutex::new(None)});
    let other_ctx=ToolContext::new(work.path()).with_read_plan_reference_port(reference.clone()).with_uri_port(other_port);
    assert_eq!(text(&execute(&other_ctx,"PLAN.md:raw",CancellationToken::new()).await.unwrap()),"other session\n");
    assert_eq!(text(&execute(&ctx,"PLAN.md:raw",CancellationToken::new()).await.unwrap()),"recreated\n");
    struct Legacy {path:PathBuf,calls:AtomicUsize}
    impl ReadPlanReferencePort for Legacy {fn read_reference_path(&self)->Option<PathBuf> {let count=self.calls.fetch_add(1,Ordering::SeqCst);assert_eq!(count,0);Some(self.path.clone())}}
    let legacy=Arc::new(Legacy{path:target.clone(),calls:AtomicUsize::new(0)});
    let legacy_ctx=ToolContext::new(work.path()).with_read_plan_reference_port(legacy.clone());
    assert!(text(&execute(&legacy_ctx,"PLAN.md:raw",CancellationToken::new()).await.unwrap()).contains("recreated"));assert_eq!(legacy.calls.load(Ordering::SeqCst),1);
    eprintln!("ARA-PLAN-URI PASS issuing-session-snapshot-and-legacy-once");
    // Legal existing image/PDF/video Host ports; no private algorithm copies.
    struct Codec(AtomicUsize);
    #[async_trait::async_trait]
    impl ara_tools::read_image::ImageCodecPort for Codec {
        async fn decoded_metadata(&self,_:&[u8],_:CancellationToken)->Result<ara_tools::read_image::DecodedImageMetadata,ToolError> {panic!("no resize metadata expected")}
        async fn decode_probe_1x1(&self,bytes:&[u8],_:CancellationToken)->Result<(),ToolError> {assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));self.0.fetch_add(1,Ordering::SeqCst);Ok(())}
        async fn encode_many(&self,_:&[u8],_:ara_tools::read_image::ImageEncodeBatch,_:CancellationToken)->Result<Vec<ara_tools::read_image::EncodedImage>,ToolError> {panic!("no resize expected")}
    }
    struct Pdf {png:PathBuf,calls:Mutex<Vec<(PathBuf,u64)>>}
    #[async_trait::async_trait]
    impl ara_tools::PdfScreenshotPort for Pdf {
        async fn render_pdf_page(&self,path:&std::path::Path,page:u64,_:CancellationToken)->Result<ara_tools::PdfScreenshot,ToolError> {
            self.calls.lock().unwrap().push((path.into(),page));
            Ok(ara_tools::PdfScreenshot{path:self.png.clone(),bytes:std::fs::read(&self.png).unwrap(),mime_type:"image/png".into(),width:1,height:1})
        }
    }
    struct Video(Mutex<Vec<ara_tools::read_video_port::MediaCommand>>);
    #[async_trait::async_trait]
    impl ara_tools::read_video_port::ReadVideoHostPort for Video {
        async fn find_binary(&self,b:ara_tools::read_video_port::MediaBinary)->Result<Option<PathBuf>,ara_tools::read_video_port::VideoError>{Ok(Some(b.name().into()))}
        async fn execute(&self,c:ara_tools::read_video_port::MediaCommand,_:Option<CancellationToken>)->Result<ara_tools::read_video_port::MediaProcessOutput,ara_tools::read_video_port::VideoError>{
            assert_eq!(c.executable,PathBuf::from("ffprobe"));self.0.lock().unwrap().push(c);
            Ok(ara_tools::read_video_port::MediaProcessOutput{stdout:br#"{"streams":[{"codec_type":"video","codec_name":"h264","width":320,"height":240,"avg_frame_rate":"30/1"}],"format":{"duration":"12","format_name":"mov,mp4"}}"#.to_vec(),stderr:vec![],exit_code:0})
        }
        async fn create_workspace(&self,_:&str)->Result<Arc<dyn ara_tools::read_video_port::ReadVideoWorkspace>,ara_tools::read_video_port::VideoError>{panic!("inspect-image mode must not extract")}
    }
    struct Inspect;
    impl ara_tools::read_image::ReadImagePolicyPort for Inspect {
        fn current_state(&self)->ara_tools::read_image::ReadImageRuntime {ara_tools::read_image::ReadImageRuntime{inspect_image_active:true,..Default::default()}}
    }
    let codec=Arc::new(Codec(AtomicUsize::new(0)));
    let png=local.path().join("image.png");
    std::fs::write(&png,base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jF1cAAAAASUVORK5CYII=").unwrap()).unwrap();
    let pdf=Arc::new(Pdf{png,calls:Mutex::new(vec![])});let video=Arc::new(Video(Mutex::new(vec![])));
    let media=ctx.clone().with_read_image_codec(codec.clone()).with_read_auto_resize_images(false).with_pdf_screenshot_port(pdf.clone())
        .with_read_video_runtime(Arc::new(ara_tools::read_video::ReadVideoRuntime::new(video.clone())));
    let svg=local.path().join("diagram.svg");std::fs::write(&svg,r#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><rect width="1" height="1"/></svg>"#).unwrap();
    locator.paths.lock().unwrap().insert("plan://current".into(),svg.clone());reset();let svg_output=execute(&media,"diagram.svg:img",CancellationToken::new()).await.unwrap();
    assert!(svg_output.content.iter().any(|b|matches!(b,UserBlock::Image(_))));assert_eq!(codec.0.load(Ordering::SeqCst),1);counts(1,1);
    let pdf_path=local.path().join("report.pdf");std::fs::write(&pdf_path,b"%PDF-1.4\n").unwrap();locator.paths.lock().unwrap().insert("plan://current".into(),pdf_path.clone());reset();
    let pdf_output=execute(&media,"report.pdf:p2-img0.png",CancellationToken::new()).await.unwrap();assert!(pdf_output.content.iter().any(|b|matches!(b,UserBlock::Image(_))));
    assert_eq!(*pdf.calls.lock().unwrap(),vec![(pdf_path,2)]);assert_eq!(codec.0.load(Ordering::SeqCst),2);counts(1,1);
    let video_media=media.clone().with_read_image_policy(Arc::new(Inspect));
    let video_path=local.path().join("clip.mp4");std::fs::write(&video_path,b"controlled video").unwrap();locator.paths.lock().unwrap().insert("plan://current".into(),video_path.clone());reset();
    let video_output=execute(&video_media,"clip.mp4:3",CancellationToken::new()).await.unwrap();counts(1,1);
    assert!(!text(&video_output).contains("read a frame"));assert_eq!(video.0.lock().unwrap().len(),1);
    assert!(video.0.lock().unwrap()[0].args.iter().any(|arg|arg==video_path.to_string_lossy().as_ref()));
    let video_directory=local.path().join("folder.mp4");std::fs::create_dir(&video_directory).unwrap();std::fs::write(video_directory.join("inside.txt"),"entry").unwrap();
    locator.paths.lock().unwrap().insert("plan://current".into(),video_directory);reset();let directory=execute(&video_media,"folder.mp4:3",CancellationToken::new()).await.unwrap();
    assert!(text(&directory).contains("inside.txt"));counts(1,1);assert_eq!(video.0.lock().unwrap().len(),1);
    eprintln!("ARA-PLAN-URI PASS saved-svg-pdf-video-directory-tails");
}
