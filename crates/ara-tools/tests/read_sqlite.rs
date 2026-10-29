//! Fixed original SQLite READ25 from OMP596f2da sqlite.test.ts.
//! Shared WRITE7 reuse accepted receipts; no WRITE mutation is repeated here.
use ara_agent::{AgentTool, ToolOutput, UpdateFn};
use ara_ai::{JsonObject, UserBlock};
use ara_tools::{ToolContext, read::ReadTool, read_sqlite::*};
use rusqlite::{Connection, types::Value};
use serde_json::json;
use std::path::Path;
use tokio_util::sync::CancellationToken;

fn text(output:&ToolOutput)->String {output.content.iter().filter_map(|c|match c {UserBlock::Text(text)=>Some(text.text.clone()),_=>None}).collect::<Vec<_>>().join("\n")}
fn args(v:serde_json::Value)->JsonObject{v.as_object().unwrap().clone()}
fn noop()->UpdateFn{std::sync::Arc::new(|_|{})}
fn tool(root:&Path)->ReadTool {ReadTool{ctx:ToolContext::new(root).with_edit(ara_edit::EditMode::Replace,true)}}
async fn read(tool:&ReadTool,path:impl AsRef<str>)->ToolOutput {
    tool.execute("sqlite-read-original",args(json!({"path":path.as_ref()})),CancellationToken::new(),noop()).await.unwrap()
}
async fn rejected(tool:&ReadTool,path:impl AsRef<str>)->String {
    tool.execute("sqlite-read-original-error",args(json!({"path":path.as_ref()})),CancellationToken::new(),noop()).await.unwrap_err().0
}
const ORIGINAL_CREATE_SQL:&str=r#"
			CREATE TABLE users (
				id INTEGER PRIMARY KEY AUTOINCREMENT,
				name TEXT NOT NULL,
				email TEXT NOT NULL,
				status TEXT NOT NULL,
				created INTEGER NOT NULL
			);
			CREATE TABLE slugs (
				slug TEXT PRIMARY KEY,
				title TEXT NOT NULL
			);
			CREATE TABLE notes (
				body TEXT NOT NULL
			);
			CREATE TABLE composite (
				team_id INTEGER NOT NULL,
				user_id INTEGER NOT NULL,
				value TEXT NOT NULL,
				PRIMARY KEY (team_id, user_id)
			);
			CREATE TABLE wide_rows (
				id INTEGER PRIMARY KEY,
				payload TEXT NOT NULL
			);
		"#;

fn fixture(path:&Path) {
    let mut db=Connection::open(path).unwrap();
    db.execute_batch(ORIGINAL_CREATE_SQL).unwrap();
    let tx=db.transaction().unwrap();
    for (name,status,n) in [("Alice","active",1),("Bob","inactive",2),("Carol","active",3),("Dave","inactive",4),("Eve","active",5),("Frank","active",6)] {
        tx.execute("INSERT INTO users (name,email,status,created) VALUES (?,?,?,?)",rusqlite::params![name,format!("{}@example.com",name.to_lowercase()),status,n]).unwrap();
    }
    tx.execute_batch("INSERT INTO slugs VALUES ('welcome','Welcome'),('about','About'); INSERT INTO notes VALUES ('First note'),('Second note'),('Third; note'); INSERT INTO composite VALUES (1,2,'pair');").unwrap();
    tx.execute("INSERT INTO wide_rows VALUES (1,?)",["x".repeat(320)]).unwrap();tx.commit().unwrap();
}
fn clean_wal(root:&Path,name:&str)->std::path::PathBuf {
    let target=root.join(name);let source=root.join(format!("{name}.source"));
    let db=Connection::open(&source).unwrap();db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE entries (id INTEGER PRIMARY KEY,value TEXT NOT NULL); INSERT INTO entries (value) VALUES ('persisted'); PRAGMA wal_checkpoint(TRUNCATE);").unwrap();drop(db);
    std::fs::copy(source,&target).unwrap();target
}

#[test]
fn original_sqlite_selector_two_cases() {
    // B-faed64a4b6
    assert_eq!(parse_path_candidates("data/app.db:users?limit=5"),vec![PathCandidate{database_path:"data/app.db".into(),sub_path:"users".into(),query_string:"limit=5".into()}]);
    assert_eq!(parse_path_candidates("data/app.sqlite"),vec![PathCandidate{database_path:"data/app.sqlite".into(),sub_path:"".into(),query_string:"".into()}]);
    // B-3fba28e3e1
    assert_eq!(parse_selector("users:42","").unwrap(),Selector::Row{table:"users".into(),key:"42".into()});
    assert_eq!(parse_selector("users","limit=2&offset=3&order=created:desc").unwrap(),Selector::Query{table:"users".into(),limit:2,offset:3.0,order:Some("created:desc".into()),where_sql:None});
    assert_eq!(parse_selector("","q=SELECT+1").unwrap(),Selector::Raw{sql:"SELECT 1".into()});
    // Supplemental Number-domain boundary: do not clamp offset to usize or wrap.
    assert!(matches!(parse_selector("users","offset=1000000000000000000000000000000").unwrap(),Selector::Query{offset,..} if offset==1e30));
}

#[tokio::test]
async fn original_sqlite_read_eighteen_cases() {
    let dir=tempfile::tempdir().unwrap();let root=dir.path();let sqlite=root.join("app.sqlite");fixture(&sqlite);
    let bytes=std::fs::read(&sqlite).unwrap();std::fs::write(root.join("app.db"),&bytes).unwrap();std::fs::write(root.join("thumbs.db"),"not sqlite\nstill text\n").unwrap();
    let tool=tool(root);
    // B-03b6cb0703
    let out=text(&read(&tool,"app.sqlite").await);assert!(out.contains("users (6 rows)"));assert!(out.contains("slugs (2 rows)"));assert!(out.contains("notes (3 rows)"));assert!(!out.contains("sqlite_sequence"));
    // B-6b8d26d819
    let wal=clean_wal(root,"clean-wal.sqlite");assert!(text(&read(&tool,wal.to_str().unwrap()).await).contains("entries (1 rows)"));
    // B-8a15ab3ac8
    let wal=clean_wal(root,"query-only-wal.sqlite");let wal=wal.to_str().unwrap();
    let rejected_mutation=rejected(&tool,format!("{wal}?q=INSERT+INTO+entries+(value)+VALUES+('mutated')")).await;assert!(!rejected_mutation.is_empty());
    assert!(text(&read(&tool,format!("{wal}?q=SELECT+COUNT(*)+AS+count+FROM+entries")).await).contains("| 1"));
    // B-57b6ec7282
    assert!(text(&read(&tool,"app.db").await).contains("users (6 rows)"));
    // B-2254686211
    assert!(text(&read(&tool,"thumbs.db").await).contains("not sqlite"));
    // B-49073b5159
    let out=text(&read(&tool,"app.sqlite:users").await);assert!(out.contains("CREATE TABLE users"));assert!(out.contains("Sample rows:"));assert!(out.contains("Alice"));
    // B-66e7bd3713
    let out=text(&read(&tool,"app.sqlite:users:2").await);assert!(out.contains("id: 2"));assert!(out.contains("name: Bob"));assert!(out.contains("email: bob@example.com"));
    // B-d1c6d8e74c
    let out=text(&read(&tool,"app.sqlite:slugs:welcome").await);assert!(out.contains("slug: welcome"));assert!(out.contains("title: Welcome"));
    // B-f0daa981cd
    assert!(text(&read(&tool,"app.sqlite:notes:1").await).contains("body: First note"));
    // B-9fd1b5f6bf
    assert!(rejected(&tool,"app.sqlite:composite:1").await.to_lowercase().contains("composite primary key"));
    // B-1dd11c7c9c
    let out=text(&read(&tool,"app.sqlite:users?limit=2&offset=1").await);assert!(out.contains("Bob"));assert!(out.contains("Carol"));assert!(!out.contains("Alice"));assert!(out.contains("append :users?limit=2&offset=3 to the database path to continue"));
    // B-99990444ff
    let out=text(&read(&tool,"app.sqlite:users?where=status='active'&order=created:desc&limit=2").await);assert!(out.contains("Frank"));assert!(out.contains("Eve"));assert!(!out.contains("Bob"));
    // B-6868107bb6
    assert!(parse_selector("users","where=1=1 LIMIT 1000000 --&limit=2&offset=0").unwrap_err().0.contains("comments or statement terminators"));
    assert!(parse_selector("users","where=status='active' LIMIT 1").unwrap_err().0.contains("LIMIT/OFFSET/UNION"));
    assert!(parse_selector("users","where=1=1; DROP TABLE users").unwrap_err().0.contains("comments or statement terminators"));
    // B-996a0c90bf
    assert!(text(&read(&tool,"app.sqlite:notes?where=body LIKE '%;%'&limit=5").await).contains("Third; note"));
    // B-40bb62a913
    assert!(rejected(&tool,"app.sqlite:users?where=1=1 LIMIT 1000000 --&limit=2&offset=0").await.contains("comments or statement terminators"));
    // B-0d4801dbdb
    assert!(rejected(&tool,"app.sqlite?q=INSERT+INTO+users+(name,email,status,created)+VALUES+('X','x@example.com','active',7)").await.to_lowercase().contains("readonly"));
    // B-b84303db89
    let cap=root.join("rawcap.sqlite");let mut db=Connection::open(&cap).unwrap();db.execute_batch("CREATE TABLE big (id INTEGER PRIMARY KEY,value TEXT NOT NULL)").unwrap();
    let tx=db.transaction().unwrap();for i in 1..=1200{tx.execute("INSERT INTO big(value) VALUES (?)",[format!("val_{i}_end")]).unwrap();}tx.commit().unwrap();drop(db);
    let out=text(&read(&tool,"rawcap.sqlite?q=SELECT * FROM big").await);assert!(out.contains("val_1000_end"));assert!(!out.contains("val_1001_end"));assert!(out.contains("Output capped at 1000 rows"));
    // B-acf4238037
    assert!(rejected(&tool,"app.sqlite:users;DROP TABLE users;").await.to_lowercase().contains("not found"));
    // Supplemental: module source metadata and cancellation, no CREATE, mutation count unchanged.
    let receipt=read(&tool,"app.sqlite:users:2").await;assert_eq!(receipt.details.as_ref().unwrap()["meta"]["source"]["type"],"path");
    let token=CancellationToken::new();token.cancel();assert!(tool.execute("cancel",args(json!({"path":"app.sqlite"})),token,noop()).await.is_err());
    assert!(open_read_connection(&root.join("must-not-create.sqlite")).is_err());assert!(!root.join("must-not-create.sqlite").exists());
    let db=open_read_connection(&sqlite).unwrap();assert_eq!(db.query_row("SELECT COUNT(*) FROM users",[],|r|r.get::<_,i64>(0)).unwrap(),6);
    drop(db);
    // Supplemental actual native REAL PK lookup, not a helper-only assertion.
    // Fixed Bun Number(0x200000000000011) rounds to 2^57+32, not 2^57.
    let radix_path=root.join("real-key.sqlite");let db=Connection::open(&radix_path).unwrap();
    db.execute_batch("CREATE TABLE real_keys (id REAL PRIMARY KEY, value TEXT NOT NULL)").unwrap();
    let target=((1u64<<57)+32) as f64;
    db.execute("INSERT INTO real_keys VALUES (?,?)",rusqlite::params![target,"single-round radix target"]).unwrap();
    db.execute("INSERT INTO real_keys VALUES (?,?)",rusqlite::params![(1u64<<57) as f64,"ties-even lower"]).unwrap();
    db.execute("INSERT INTO real_keys VALUES (?,?)",rusqlite::params![((1u64<<57)+64) as f64,"ties-odd upper"]).unwrap();
    drop(db);
    for key in ["0x200000000000011", "0X200000000000011", "0o10000000000000000021", "0b1000000000000000000000000000000000000000000000000000010001"] {
        assert!(text(&read(&tool,format!("real-key.sqlite:real_keys:{key}")).await).contains("value: single-round radix target"),"{key}");
    }
    let long_key=format!("0x{}200000000000011","0".repeat(2000));
    assert!(text(&read(&tool,format!("real-key.sqlite:real_keys:{long_key}")).await).contains("value: single-round radix target"));
    for (key,value) in [("0x200000000000010","ties-even lower"),("0x200000000000030","ties-odd upper")] {
        assert!(text(&read(&tool,format!("real-key.sqlite:real_keys:{key}")).await).contains(value),"{key}");
    }
    for key in [format!("0x{}","f".repeat(256)),format!("0x{}7","f".repeat(255)),format!("0b1{}","0".repeat(1024))] {
        assert!(text(&read(&tool,format!("real-key.sqlite:real_keys:{key}")).await).starts_with("No row found in table 'real_keys'"));
    }
}

#[test]
fn original_sqlite_width_and_vertical_two_cases() {
    // B-72be80b961
    let rendered=render_table(&AsciiReadTextMetrics,&["id".into(),"payload".into()],&[vec![("id".into(),Value::Integer(1)),("payload".into(),Value::Text("x".repeat(320)))]]).unwrap();
    for line in rendered.split('\n'){assert!(AsciiReadTextMetrics.width(line).unwrap()<=120);}
    // B-96b370071f
    let mut columns=vec!["_id".into()];let mut row=vec![("_id".into(),Value::Integer(7))];
    for i in 1..=32{columns.push(format!("col_{i}"));row.push((format!("col_{i}"),Value::Text(format!("value_{i}"))));}
    let rendered=render_table(&AsciiReadTextMetrics,&columns,&[row]).unwrap();
    assert!(!regex::Regex::new("…(?: \\| …){2,}").unwrap().is_match(&rendered));
    assert!(rendered.contains("── Row 1 ──"));assert!(rendered.contains("_id   : 7"));assert!(rendered.contains("col_1 : value_1"));assert!(rendered.contains("col_32: value_32"));
    for line in rendered.split('\n'){assert!(AsciiReadTextMetrics.width(line).unwrap()<=120);}
    // Restricted fallback refuses ANSI/control/arbitraryUnicode rather than inventing width.
    for value in ["\u{1b}[31mred\u{1b}[0m","\u{1b}]8;;https://example.com\u{7}x","汉字","👨‍👩‍👧","\t","\n"]{assert!(AsciiReadTextMetrics.width(value).is_err());}
}

#[test]
fn original_sqlite_row_counts_three_cases() {
    fn build(analyze:bool)->Connection{let db=Connection::open_in_memory().unwrap();db.execute_batch("CREATE TABLE big (id INTEGER PRIMARY KEY,v TEXT NOT NULL);CREATE TABLE small(id INTEGER PRIMARY KEY)").unwrap();for _ in 0..10{db.execute("INSERT INTO big(v) VALUES ('x')",[]).unwrap();}for _ in 0..2{db.execute("INSERT INTO small DEFAULT VALUES",[]).unwrap();}if analyze{db.execute_batch("ANALYZE").unwrap();}db}
    let base=build(false);let analyzed=build(true);
    // B-e687eb8bc3
    let text=render_table_list(&AsciiReadTextMetrics,&list_tables(&base,100).unwrap()).unwrap();assert!(text.contains("big (10 rows)"));assert!(text.contains("small (2 rows)"));
    // B-f8a29e0f1f
    let text=render_table_list(&AsciiReadTextMetrics,&list_tables(&analyzed,5).unwrap()).unwrap();assert!(text.contains("big (~10 rows)"));assert!(text.contains("small (2 rows)"));
    // B-8ff28926f9
    let text=render_table_list(&AsciiReadTextMetrics,&list_tables(&base,3).unwrap()).unwrap();assert!(text.contains("big (3+ rows)"));assert!(text.contains("small (2 rows)"));
}
