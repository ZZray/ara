//! Original fixed OMP sqlite.test.ts WRITE contracts (MIT). Their upstream
//! surface remains CA-TOOL-READ; exercising them does not accept that surface.
use ara_agent::{AgentTool, ToolError, ToolOutput};
use ara_ai::{JsonObject, UserBlock};
use ara_tools::ToolContext;
use ara_tools::edit_host::{EditHostPathPolicy, EditHostPort};
use ara_tools::write::WriteTool;
use rusqlite::{Connection, OptionalExtension};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use tokio_util::sync::CancellationToken;

fn fixture() -> (tempfile::TempDir, PathBuf) {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    let bytes=BYTES.get_or_init(||{
        let root=tempfile::tempdir().unwrap();let path=root.path().join("fixture.sqlite");
        let db=Connection::open(&path).unwrap();
        db.execute_batch("BEGIN;
            CREATE TABLE users(id INTEGER PRIMARY KEY AUTOINCREMENT,name TEXT NOT NULL,email TEXT NOT NULL,status TEXT NOT NULL,created INTEGER NOT NULL);
            CREATE TABLE slugs(slug TEXT PRIMARY KEY,title TEXT NOT NULL);
            CREATE TABLE notes(body TEXT NOT NULL);
            CREATE TABLE composite(team_id INTEGER NOT NULL,user_id INTEGER NOT NULL,value TEXT NOT NULL,PRIMARY KEY(team_id,user_id));
            CREATE TABLE scalars(id INTEGER PRIMARY KEY,value DEFAULT 'default');
            CREATE TABLE numeric(key REAL PRIMARY KEY,value TEXT);
            CREATE TABLE quoted(\"a\"\"b\" TEXT);
            INSERT INTO users(name,email,status,created) VALUES
                ('Alice','alice@example.com','active',1),('Bob','bob@example.com','inactive',2),
                ('Carol','carol@example.com','active',3),('Dave','dave@example.com','inactive',4),
                ('Eve','eve@example.com','active',5),('Frank','frank@example.com','active',6);
            INSERT INTO slugs VALUES ('welcome','Welcome'),('about','About');
            INSERT INTO notes VALUES ('First note'),('Second note'),('Third; note');
            INSERT INTO composite VALUES(1,2,'pair');
            INSERT INTO numeric VALUES(16,'old');COMMIT;").unwrap();
        drop(db);std::fs::read(path).unwrap()
    });
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("app.sqlite");
    std::fs::write(&path, bytes).unwrap();
    (root, path)
}
fn arguments(path: &str, content: &str) -> JsonObject {
    json!({"path":path,"content":content}).as_object().unwrap().clone()
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
async fn write(ctx: ToolContext, path: &str, content: &str) -> Result<ToolOutput, ToolError> {
    WriteTool::new(ctx).execute("write", arguments(path, content), CancellationToken::new(), Arc::new(|_| {})).await
}
fn email(path: &Path, id: i64) -> Option<String> {
    Connection::open(path)
        .unwrap()
        .query_row("SELECT email FROM users WHERE id=?", [id], |row| row.get(0))
        .optional()
        .unwrap()
}

#[tokio::test]
async fn original_B_95eedaf3b5() {
    let (root, path) = fixture();
    let output = write(
        ToolContext::new(root.path()),
        "app.sqlite:users",
        "{ name: 'Grace', email: 'grace@example.com', status: 'active', created: 7 }",
    )
    .await
    .unwrap();
    let db = Connection::open(&path).unwrap();
    let row: (String, String) = db
        .query_row("SELECT name,email FROM users WHERE email='grace@example.com'", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(row, ("Grace".into(), "grace@example.com".into()));
    assert_eq!(text(&output), "Inserted row into users");
    assert_eq!(output.details.as_ref().unwrap()["resolvedPath"], path.to_string_lossy().as_ref());
    assert_eq!(output.details.as_ref().unwrap()["meta"]["source"]["type"], "path");
}
#[tokio::test]
async fn original_B_a453467663() {
    let (root, path) = fixture();
    write(ToolContext::new(root.path()), "app.sqlite:users:2", "{ email: 'bob+new@example.com' }").await.unwrap();
    assert_eq!(email(&path, 2).as_deref(), Some("bob+new@example.com"));
}
#[tokio::test]
async fn original_B_5d206d8c87() {
    let (root, path) = fixture();
    write(ToolContext::new(root.path()), "app.sqlite:users:2", "   ").await.unwrap();
    assert_eq!(
        Connection::open(&path)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM users", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        5
    );
    assert_eq!(email(&path, 2), None);
}
struct Host {
    policy: EditHostPathPolicy,
    invalidations: Mutex<Vec<PathBuf>>,
}
impl EditHostPort for Host {
    fn path_policy(&self) -> EditHostPathPolicy {
        self.policy.clone()
    }
    fn invalidate_after_write(&self, path: &Path) {
        self.invalidations.lock().unwrap().push(path.to_owned());
    }
}
#[tokio::test]
async fn original_B_637cd0373d() {
    let (root, path) = fixture();
    let before = std::fs::read(&path).unwrap();
    let host = Arc::new(Host {
        policy: EditHostPathPolicy {
            plan_active: true,
            plan_file_path: Some(root.path().join("plan.md").to_string_lossy().into()),
            ..Default::default()
        },
        invalidations: Mutex::new(Vec::new()),
    });
    let error = write(
        ToolContext::new(root.path()).with_edit_host(host.clone()),
        "app.sqlite:users:1",
        "{ email: 'blocked@example.com' }",
    )
    .await
    .unwrap_err();
    assert!(error.0.to_lowercase().contains("plan mode"), "{}", error.0);
    assert_eq!(std::fs::read(path).unwrap(), before);
    assert!(host.invalidations.lock().unwrap().is_empty());
}
#[tokio::test]
async fn original_B_ebea72a73f() {
    let (root, path) = fixture();
    let before = std::fs::read(&path).unwrap();
    let error = write(ToolContext::new(root.path()), "app.sqlite:missing", "{ value: 1 }").await.unwrap_err();
    assert!(error.0.to_lowercase().contains("not found"), "{}", error.0);
    assert_eq!(std::fs::read(path).unwrap(), before);
}
#[tokio::test]
async fn original_B_3f61e8e2a8() {
    let (root, _) = fixture();
    let path = root.path().join("missing.sqlite");
    let error = write(ToolContext::new(root.path()), "missing.sqlite:users", "{ name: 'Nope' }").await.unwrap_err();
    assert!(error.0.to_lowercase().contains("not found"), "{}", error.0);
    assert!(!path.exists());
}
#[tokio::test]
async fn original_B_862587eb40() {
    let (root, path) = fixture();
    let before = std::fs::read(&path).unwrap();
    let error = write(ToolContext::new(root.path()), "app.sqlite:users", "{ bogus: 1 }").await.unwrap_err();
    assert!(error.0.contains("no column named 'bogus'"), "{}", error.0);
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[tokio::test]
async fn fixed_sqlite_write_source_boundaries_and_builtin_memory() {
    // The same coarse group has a bounded child mode, so HOME changes never
    // race with another test or redirect writes into the real user's home.
    if let Some(cwd) = std::env::var_os("ARA_SQLITE_HOME_WITNESS_CWD") {
        let cwd = PathBuf::from(cwd);
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        let cwd_before = std::fs::read(cwd.join("app.sqlite")).unwrap();
        write(ToolContext::new(&cwd), "~/app.sqlite:users:1", "{email:'home-only@example.com'}").await.unwrap();
        assert_eq!(email(&home.join("app.sqlite"), 1).as_deref(), Some("home-only@example.com"));
        assert_eq!(std::fs::read(cwd.join("app.sqlite")).unwrap(), cwd_before);
        return;
    }
    let (root, path) = fixture();
    let (home_root, _) = fixture();
    let cwd_before = std::fs::read(&path).unwrap();
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap());
    child
        .arg("--exact")
        .arg("fixed_sqlite_write_source_boundaries_and_builtin_memory")
        .arg("--nocapture")
        .env("HOME", home_root.path())
        .env("ARA_SQLITE_HOME_WITNESS_CWD", root.path())
        .kill_on_drop(true);
    let child = tokio::time::timeout(std::time::Duration::from_secs(15), child.output())
        .await
        .expect("bounded HOME witness timed out")
        .unwrap();
    assert!(
        child.status.success(),
        "HOME witness: {}\n{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );

    assert_eq!(email(&home_root.path().join("app.sqlite"), 1).as_deref(), Some("home-only@example.com"));
    assert_eq!(std::fs::read(&path).unwrap(), cwd_before);

    // Populate the actual shared scan, add an out-of-band sibling, then prove
    // a no-Host SQL write invalidates that scan without bumping EditStore.
    let request = ara_tools::engine::GlobRequest {
        root: root.path(),
        pattern: "*",
        recursive: true,
        include_hidden: true,
        use_gitignore: false,
        max_results: usize::MAX,
    };
    let budget = ara_tools::engine::Budget {
        cancel: CancellationToken::new(),
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(3),
    };
    assert!(!ara_tools::engine::glob_with_cache(&request, &budget, true).unwrap().is_empty());
    std::fs::write(root.path().join("scan-witness.txt"), "external sibling").unwrap();
    assert!(
        !ara_tools::engine::glob_with_cache(&request, &budget, true)
            .unwrap()
            .iter()
            .any(|entry| entry.relative == "scan-witness.txt"),
        "scan was not cached"
    );
    write(ToolContext::new(root.path()), "app.sqlite:scalars", "{id:99,value:'cache'}").await.unwrap();
    assert!(
        ara_tools::engine::glob_with_cache(&request, &budget, true)
            .unwrap()
            .iter()
            .any(|entry| entry.relative == "scan-witness.txt")
    );
    let host = Arc::new(Host { policy: Default::default(), invalidations: Mutex::new(Vec::new()) });
    let ctx = ToolContext::new(root.path()).with_edit_host(host.clone());
    let cases = [
        ("null", "null", ""),
        ("true", "integer", "31"),
        ("false", "integer", "30"),
        ("1.0", "integer", "31"),
        ("-0", "real", "302E30"),
        ("\u{feff}-0", "real", "302E30"),
        ("-0\u{feff}", "real", "302E30"),
        ("9007199254740993", "real", "393030373139393235343734303939322E30"),
        ("NaN", "null", ""),
        ("Infinity", "real", "496E66"),
        ("-Infinity", "real", "2D496E66"),
        ("0x10", "integer", "3136"),
        ("'\\uD800'", "text", "EDA080"),
        ("'x\\uD800y'", "text", "78F09081B9"),
        ("'\\uDC00'", "text", "EDB080"),
        ("'\\uD83D\\uDE00'", "text", "F09F9880"),
        ("'\\u0000'", "text", "00"),
    ];
    for (index, (source, kind, hex)) in cases.iter().enumerate() {
        write(ctx.clone(), "app.sqlite:scalars", &format!("{{id:{},value:{source}}}", index + 1)).await.unwrap();
        let actual: (String, String) = Connection::open(&path)
            .unwrap()
            .query_row("SELECT typeof(value),hex(value) FROM scalars WHERE id=?", [index as i64 + 1], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(actual.0, *kind, "stored type for {source}");
        if *kind == "real" {
            // hex(REAL) hex-encodes SQLite's numeric-to-text conversion, not
            // the stored number. SQLite 3.50.2 and Bun's 3.53.2 use different
            // large-number formats; compare the binding's actual binary64.
            let expected_bits = match *source {
                "9007199254740993" => 0x4340000000000000u64,
                "Infinity" => f64::INFINITY.to_bits(),
                "-Infinity" => f64::NEG_INFINITY.to_bits(),
                "-0" | "\u{feff}-0" | "-0\u{feff}" => (-0.0f64).to_bits(),
                other => panic!("unmapped REAL oracle: {other}"),
            };
            let stored: f64 = Connection::open(&path)
                .unwrap()
                .query_row("SELECT value FROM scalars WHERE id=?", [index as i64 + 1], |row| row.get(0))
                .unwrap();
            assert_eq!(stored.to_bits(), expected_bits, "stored binary64 for {source}");
        } else {
            assert_eq!(actual.1, *hex, "stored scalar bytes for {source}");
        }
    }
    for (source, bits) in [
        ("\u{feff}0x20000000000001f", 0x4380000000000001u64),
        ("\u{feff}123456789012345678901234567890123456789012345", 0x491624db949eb59eu64),
    ] {
        write(ctx.clone(), "app.sqlite:scalars:99", &format!("{{value:{source}}}")).await.unwrap();
        let number: f64 = Connection::open(&path)
            .unwrap()
            .query_row("SELECT value FROM scalars WHERE id=99", [], |row| row.get(0))
            .unwrap();
        assert_eq!(number.to_bits(), bits, "FEFF {source}");
    }
    let numeric_db = Connection::open(&path).unwrap();
    numeric_db.execute("INSERT INTO numeric VALUES (?,'wrong')", [f64::from_bits(0x4380000000000000)]).unwrap();
    numeric_db.execute("INSERT INTO numeric VALUES (?,'correct')", [f64::from_bits(0x4380000000000001)]).unwrap();
    drop(numeric_db);
    for key in
        ["0x20000000000001f", "0o10000000000000000037", "0b1000000000000000000000000000000000000000000000000000011111"]
    {
        write(ctx.clone(), &format!("app.sqlite:numeric:{key}"), "{value:'matched'}").await.unwrap();
        let numeric_db = Connection::open(&path).unwrap();
        let actual: String = numeric_db
            .query_row("SELECT value FROM numeric WHERE key=?", [f64::from_bits(0x4380000000000001)], |row| row.get(0))
            .unwrap();
        assert_eq!(actual, "matched", "{key}");
        let wrong: String = numeric_db
            .query_row("SELECT value FROM numeric WHERE key=?", [f64::from_bits(0x4380000000000000)], |row| row.get(0))
            .unwrap();
        assert_eq!(wrong, "wrong", "{key}");
    }
    write(ctx.clone(), "app.sqlite:scalars", "{}").await.unwrap();
    write(ctx.clone(), "app.sqlite:slugs:welcome", "{/* comment */ title:'Hello',}").await.unwrap();
    write(ctx.clone(), "app.sqlite:notes:1", "{body:'updated'}").await.unwrap();
    write(ctx.clone(), "app.sqlite:numeric:0x10", "{value:'new'}").await.unwrap();
    write(ctx.clone(), "app.sqlite:quoted", "{'a\"b':'quoted'}").await.unwrap();
    write(ctx.clone(), "app.sqlite:scalars", "{id:9007199254740991,value:'large'}").await.unwrap();
    write(ctx.clone(), "app.sqlite:scalars:9007199254740991", "{value:'changed'}").await.unwrap();
    let db = Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("SELECT title FROM slugs WHERE slug='welcome'", [], |row| { row.get::<_, String>(0) }).unwrap(),
        "Hello"
    );
    assert_eq!(
        db.query_row("SELECT body FROM notes WHERE rowid=1", [], |row| row.get::<_, String>(0)).unwrap(),
        "updated"
    );
    assert_eq!(
        db.query_row("SELECT value FROM numeric WHERE key=16", [], |row| row.get::<_, String>(0)).unwrap(),
        "new"
    );
    assert_eq!(
        db.query_row("SELECT value FROM scalars WHERE id=9007199254740991", [], |row| row.get::<_, String>(0)).unwrap(),
        "changed"
    );
    db.execute("INSERT INTO scalars VALUES (9007199254740993,'bigint')", []).unwrap();
    drop(db);
    write(ctx.clone(), "app.sqlite:scalars:9007199254740993", "{value:'bigint updated'}").await.unwrap();
    assert_eq!(
        Connection::open(&path)
            .unwrap()
            .query_row("SELECT value FROM scalars WHERE id=9007199254740993", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "bigint updated"
    );
    for (target, content, expected) in [
        ("app.sqlite:scalars", "[]", "JSON object"),
        ("app.sqlite:scalars", "{value:[]}", "JSON scalar"),
        ("app.sqlite:scalars", "{value: {}}", "JSON scalar"),
        ("app.sqlite:scalars", "{value:'\\1'}", "valid JSON5"),
        ("app.sqlite:scalars", "{value:'\\uZZZZ'}", "valid JSON5"),
        ("app.sqlite:scalars", "{value:'unterminated}", "valid JSON5"),
        ("app.sqlite:scalars", "{value:01}", "valid JSON5"),
        ("app.sqlite:scalars", " ", "row key"),
        ("app.sqlite:scalars:1", "{}", "at least one column"),
        ("app.sqlite:composite:1", "{value:'x'}", "composite primary key"),
        ("app.sqlite:scalars?limit=1", "{}", "query parameters"),
        ("app.sqlite:scalars:", "{}", "non-empty row key"),
        ("app.sqlite:missing", "{", "valid JSON5"),
        ("app.sqlite:users:notint", "{bogus:1}", "no column named 'bogus'"),
        ("app.sqlite:users:notint", "{}", "at least one column"),
        ("app.sqlite:users:notint", "{email:[]}", "JSON scalar"),
        ("app.sqlite:notes:notint", "{bogus:1}", "no column named 'bogus'"),
        ("app.sqlite:notes:notint", "{}", "at least one column"),
        ("app.sqlite:notes:notint", "{body:[]}", "JSON scalar"),
    ] {
        let before = std::fs::read(&path).unwrap();
        let count = host.invalidations.lock().unwrap().len();
        let error = write(ctx.clone(), target, content).await.unwrap_err();
        assert!(error.0.contains(expected), "{target} {content}: {}", error.0);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(host.invalidations.lock().unwrap().len(), count);
    }
    let before = std::fs::read(&path).unwrap();
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let updates = Arc::new(Mutex::new(Vec::new()));
    let collected = updates.clone();
    let error = WriteTool::new(ctx.clone())
        .execute(
            "cancel",
            arguments("app.sqlite:users:1", "{email:'cancelled'}"),
            cancel,
            Arc::new(move |output| {
                collected.lock().unwrap().push(text(&output));
                token.cancel();
            }),
        )
        .await
        .unwrap_err();
    assert!(error.0.contains("Cancelled"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(updates.lock().unwrap().len(), 1);
    let error = write(ctx, "memory://latest", "overwrite").await.unwrap_err();
    assert_eq!(error.0, "memory:// URLs are read-only for write; use the protocol-specific tool for mutations.");
}
