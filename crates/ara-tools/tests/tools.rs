use ara_agent::{AgentTool, ToolOutput, UpdateFn};
use ara_ai::{JsonObject, UserBlock};
use ara_tools::*;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

fn args(v: Value) -> JsonObject {
    v.as_object().unwrap().clone()
}

fn noop() -> UpdateFn {
    Arc::new(|_| {})
}

fn text(o: &ToolOutput) -> String {
    o.content
        .iter()
        .filter_map(|b| if let UserBlock::Text(t) = b { Some(t.text.clone()) } else { None })
        .collect::<Vec<_>>()
        .join("\n")
}

fn tools(dir: &std::path::Path) -> (read::ReadTool, write::WriteTool, bash::BashTool) {
    let ctx = ToolContext::new(dir);
    (read::ReadTool { ctx: ctx.clone() }, write::WriteTool { ctx: ctx.clone() }, bash::BashTool { ctx })
}

#[tokio::test]
async fn read_ranges_numbers_and_limits() {
    let dir = tempfile::tempdir().unwrap();
    let (read, _, _) = tools(dir.path());
    std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\nfour\nfive\n").unwrap();
    let r = |p: &str| {
        let read = &read;
        let p = p.to_string();
        async move { read.execute("c", args(json!({"path": p})), CancellationToken::new(), noop()).await }
    };
    assert_eq!(text(&r("a.txt").await.unwrap()), "one\ntwo\nthree\nfour\nfive");
    assert_eq!(text(&r("a.txt:2-3").await.unwrap()), "two\nthree\n\n[2 more lines in file. Use :4 to continue]");
    assert_eq!(text(&r("a.txt:-2").await.unwrap()), "four\nfive");
    assert_eq!(text(&r("a.txt:4+5").await.unwrap()), "four\nfive");
    assert_eq!(
        text(&r("a.txt:9").await.unwrap()),
        "Line 9 is beyond end of file (5 lines total). Use :1 to read from the start, or :5 to read the last line."
    );
    assert_eq!(r("missing.txt").await.unwrap_err().0, "File not found: missing.txt");

    let numbered = read::ReadTool { ctx: ToolContext { cwd: dir.path().into(), line_numbers: true } };
    let out =
        numbered.execute("c", args(json!({"path": "a.txt:2-3"})), CancellationToken::new(), noop()).await.unwrap();
    assert!(text(&out).starts_with("2|two\n3|three"));
    let raw =
        numbered.execute("c", args(json!({"path": "a.txt:raw:2-2"})), CancellationToken::new(), noop()).await.unwrap();
    assert!(text(&raw).starts_with("two\n"), "raw drops line numbers");

    let big: String = (1..=5000).map(|i| format!("{i}\n")).collect();
    std::fs::write(dir.path().join("big.txt"), big).unwrap();
    let out = r("big.txt").await.unwrap();
    assert!(text(&out).ends_with("\n\n[2000 more lines in file. Use :3001 to continue]"));
    assert_eq!(out.details.unwrap()["truncation"]["truncatedBy"], json!("lines"));
    let wide = "x".repeat(1000) + "\n";
    std::fs::write(dir.path().join("wide.txt"), wide.repeat(100)).unwrap();
    let out = r("wide.txt").await.unwrap();
    assert!(text(&out).len() <= DEFAULT_MAX_BYTES + 100);
    assert_eq!(out.details.unwrap()["truncation"]["truncatedBy"], json!("bytes"));
}

#[tokio::test]
async fn read_directories_binaries_images() {
    let dir = tempfile::tempdir().unwrap();
    let (read, _, _) = tools(dir.path());
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("b.bin"), [0u8, 159, 146, 150, 0, 1]).unwrap();
    std::fs::write(dir.path().join("p.png"), [137u8, 80, 78, 71]).unwrap();
    let r = |p: &str| read.execute("c", args(json!({"path": p})), CancellationToken::new(), noop());
    assert_eq!(text(&r(".").await.unwrap()), "b.bin\np.png\nsub/");
    assert!(
        text(&r("b.bin").await.unwrap()).starts_with("[Cannot read binary file 'b.bin' (6B); not valid UTF-8 text.")
    );
    let img = r("p.png").await.unwrap();
    assert!(matches!(&img.content[1], UserBlock::Image(i) if i.mime_type == "image/png" && i.data == "iVBORw=="));
}

#[tokio::test]
async fn write_creates_dirs_counts_utf16_and_marks_shebang() {
    let dir = tempfile::tempdir().unwrap();
    let (_, write, _) = tools(dir.path());
    let w =
        |p: &str, c: &str| write.execute("c", args(json!({"path": p, "content": c})), CancellationToken::new(), noop());
    let out = w("deep/nested/x.txt", "héllo😀").await.unwrap();
    assert_eq!(text(&out), "Successfully wrote 7 bytes to deep/nested/x.txt");
    assert_eq!(std::fs::read_to_string(dir.path().join("deep/nested/x.txt")).unwrap(), "héllo😀");
    let out = w("run.sh", "#!/bin/sh\necho hi\n").await.unwrap();
    assert_eq!(text(&out), format!("Successfully wrote 18 bytes to run.sh\n{}", write::EXECUTABLE_NOTICE));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(std::fs::metadata(dir.path().join("run.sh")).unwrap().permissions().mode() & 0o111, 0);
    }
    let again = w("run.sh", "#!/bin/sh\necho changed\n").await.unwrap();
    assert!(!text(&again).contains("chmod"), "existing file keeps its mode");
    assert!(w("deep", "x").await.unwrap_err().0.contains("is a directory"));
}

#[tokio::test]
async fn bash_success_error_env_cwd() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    let (_, _, bash) = tools(dir.path());
    let b = |v: Value| bash.execute("c", args(v), CancellationToken::new(), noop());
    let out = b(json!({"command": "echo out; echo err 1>&2"})).await.unwrap();
    assert!(!out.is_error);
    let t = text(&out);
    assert!(t.contains("out") && t.contains("err"), "{t}");
    assert_eq!(out.details.as_ref().unwrap()["timeoutSeconds"], json!(300));
    let out = b(json!({"command": "echo nope; exit 3"})).await.unwrap();
    assert!(out.is_error);
    assert_eq!(text(&out), "nope\n\nCommand exited with code 3");
    assert_eq!(out.details.unwrap()["exitCode"], json!(3));
    assert_eq!(text(&b(json!({"command": "true"})).await.unwrap()), "(no output)");
    let out =
        b(json!({"command": "echo $ARA_X $PAGER $GIT_TERMINAL_PROMPT; pwd", "env": {"ARA_X": "set"}, "cwd": "sub"}))
            .await
            .unwrap();
    let t = text(&out);
    assert!(t.starts_with("set cat 0\n") && t.ends_with("/sub"), "{t}");
    assert!(
        b(json!({"command": "true", "cwd": "nope"}))
            .await
            .unwrap_err()
            .0
            .starts_with("Working directory does not exist")
    );
    let out = b(json!({"command": "seq 1 20000"})).await.unwrap();
    let t = text(&out);
    assert!(t.ends_with("of 20000]") && t.contains("\n20000\n"), "tail kept");
}

#[tokio::test]
async fn bash_timeout_kills_process_group() {
    let dir = tempfile::tempdir().unwrap();
    let (_, _, bash) = tools(dir.path());
    let marker = dir.path().join("survived");
    let started = Instant::now();
    let cmd = format!("(sleep 3; touch {}) & echo started; wait", marker.display());
    let out =
        bash.execute("c", args(json!({"command": cmd, "timeout": 1})), CancellationToken::new(), noop()).await.unwrap();
    assert!(started.elapsed() < Duration::from_millis(2800), "{:?}", started.elapsed());
    assert!(out.is_error);
    assert_eq!(text(&out), "started\n\n[Command timed out after 1 seconds]");
    assert_eq!(out.details.unwrap()["timedOut"], json!(true));
    tokio::time::sleep(Duration::from_millis(3000)).await;
    assert!(!marker.exists(), "background child in the group was killed");
}

#[tokio::test]
async fn bash_cancel_aborts_and_streams_updates() {
    let dir = tempfile::tempdir().unwrap();
    let (_, _, bash) = tools(dir.path());
    let updates = Arc::new(Mutex::new(Vec::<String>::new()));
    let u2 = updates.clone();
    let update: UpdateFn = Arc::new(move |o| u2.lock().unwrap().push(text(&o)));
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    let u3 = updates.clone();
    tokio::spawn(async move {
        // Cancel only after a live partial update has been observed.
        loop {
            if u3.lock().unwrap().iter().any(|t| t.contains("tick 1")) {
                c2.cancel();
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    let started = Instant::now();
    let err = bash
        .execute(
            "c",
            args(json!({"command": "for i in 1 2 3 4 5 6 7 8 9; do echo tick $i; sleep 1; done"})),
            cancel,
            update,
        )
        .await
        .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(4));
    assert!(err.0.starts_with("tick 1") && err.0.ends_with("[Command aborted]"), "{}", err.0);
}

#[tokio::test]
async fn builtin_tool_set() {
    let names: Vec<String> = builtin_tools(ToolContext::new(".")).iter().map(|t| t.definition().name).collect();
    assert_eq!(names, vec!["read", "write", "bash"]);
}

#[tokio::test]
async fn bash_keeps_stream_order_and_reaps_background_children() {
    let dir = tempfile::tempdir().unwrap();
    let (_, _, bash) = tools(dir.path());
    let b = |v: Value| bash.execute("c", args(v), CancellationToken::new(), noop());
    let out = b(json!({"command": "printf 'a\\n'; printf 'b\\n' >&2; printf 'c\\n'"})).await.unwrap();
    assert_eq!(text(&out), "a\nb\nc");
    let marker = dir.path().join("bg-survived");
    let started = Instant::now();
    let out = b(json!({"command": format!("(sleep 2; touch {}) & echo hi", marker.display())})).await.unwrap();
    assert!(started.elapsed() < Duration::from_millis(1500), "{:?}", started.elapsed());
    assert_eq!(text(&out), "hi");
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(!marker.exists(), "background child killed with the call");
    let out = b(json!({"command": "kill -9 $$"})).await.unwrap();
    assert!(out.is_error);
    assert!(text(&out).ends_with("Command exited with code 137"), "{}", text(&out));
    let out = b(json!({"command": "head -c 30000000 /dev/zero | tr '\\0' 'x' | fold -w 100"})).await.unwrap();
    assert!(text(&out).len() < DEFAULT_MAX_BYTES + 200);
    assert!(text(&out).ends_with("of 300000]"), "{}", &text(&out)[text(&out).len() - 60..]);
}

#[tokio::test]
async fn dropping_a_bash_call_kills_its_group() {
    let dir = tempfile::tempdir().unwrap();
    let (_, _, bash) = tools(dir.path());
    let marker = dir.path().join("after-drop");
    let cmd = format!("sleep 2; touch {}", marker.display());
    let r = tokio::time::timeout(
        Duration::from_millis(300),
        bash.execute("c", args(json!({"command": cmd})), CancellationToken::new(), noop()),
    )
    .await;
    assert!(r.is_err());
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(!marker.exists(), "group killed by the drop guard");
}

#[tokio::test]
async fn read_bounds_and_robustness() {
    let dir = tempfile::tempdir().unwrap();
    let (read, _, _) = tools(dir.path());
    let r = |p: &str| read.execute("c", args(json!({"path": p})), CancellationToken::new(), noop());
    std::fs::write(dir.path().join("one.txt"), "y".repeat(200_000) + "\nsecond\n").unwrap();
    let out = text(&r("one.txt").await.unwrap());
    assert!(out.len() < DEFAULT_MAX_BYTES + 300, "{}", out.len());
    assert!(
        out.contains("[Line 1 is 195.3KB, exceeds 50.0KB limit. Showing its first 50.0KB.]"),
        "{}",
        &out[out.len() - 200..]
    );
    assert!(out.ends_with("[1 more lines in file. Use :2 to continue]"));
    let mut late = "ok ok ok ok ok ok ok\n".repeat(1000).into_bytes();
    late.extend_from_slice(&[0xE9, b'\n']);
    std::fs::write(dir.path().join("lat.txt"), late).unwrap();
    let out = text(&r("lat.txt").await.unwrap());
    assert!(out.ends_with("\u{FFFD}"), "late invalid byte decoded lossily");
    assert!(r("one.txt:0").await.unwrap_err().0.starts_with("Invalid selector ':0' on 'one.txt'"));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(dir.path().join("nowhere"), dir.path().join("log:1-2")).unwrap();
        assert_eq!(r("log:1-2").await.unwrap_err().0, "File not found: log:1-2", "dangling symlink is a literal path");
        let fifo = dir.path().join("pipe");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
        assert_eq!(r("pipe").await.unwrap_err().0, "Cannot read pipe: not a regular file");
    }
    let home = std::env::var("HOME").unwrap();
    let ctx = ToolContext::new(dir.path());
    assert_eq!(ctx.resolve("~/x/../y"), std::path::PathBuf::from(home).join("y"));
    assert_eq!(ctx.resolve("a/./b/../c"), dir.path().join("a/c"));
}
