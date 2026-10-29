//! Real child-process fixture for the existing EDIT/WRITE coarse groups.
//! This is a controlled server, not a production language-service substitute.
use serde_json::{Value, json};
use std::{collections::HashMap, io, path::PathBuf, sync::Arc, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

#[derive(Clone)]
struct Document {
    text: String,
    version: i64,
}
#[derive(Clone)]
struct Fixture {
    output: Arc<Mutex<tokio::io::Stdout>>,
    documents: Arc<Mutex<HashMap<String, Document>>>,
    mode: String,
    log: Option<PathBuf>,
}
impl Fixture {
    fn log(&self, message: &Value) {
        use std::io::Write as _;
        if let Some(path) = &self.log {
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                let line = format!("{}\n", json!({"pid":std::process::id(),"frame":message}));
                let _ = file.write_all(line.as_bytes());
            }
        }
    }
    async fn send(&self, message: Value) -> io::Result<()> {
        self.log(&json!({"direction":"out","message":message}));
        let bytes = serde_json::to_vec(&message)?;
        let mut output = self.output.lock().await;
        output.write_all(format!("Content-Length: {}\r\n\r\n", bytes.len()).as_bytes()).await?;
        output.write_all(&bytes).await?;
        output.flush().await
    }
    async fn publish(&self, uri: String, document: Document) -> io::Result<()> {
        tokio::time::sleep(delay(&document.text)).await;
        let mut params = json!({"uri":uri,"diagnostics":diagnostics(&document.text)});
        if self.mode != "unversioned" {
            params["version"] = json!(document.version);
        }
        self.send(json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":params})).await
    }
    async fn handle(&self, message: Value) -> io::Result<bool> {
        self.log(&json!({"direction":"in","message":message}));
        let method = message["method"].as_str().unwrap_or_default();
        if method.is_empty() {
            return Ok(true);
        }
        let params = &message["params"];
        let id = message.get("id").cloned();
        match method {
            "initialize" => {
                if self.mode == "init-hang" {
                    return Ok(true);
                }
                if self.mode == "malformed-frame" {
                    self.output.lock().await.write_all(b"Content-Length: not-a-number\r\n\r\n{}").await?;
                    return Ok(false);
                }
                let mut capabilities = json!({"textDocumentSync":{"openClose":true,"change":2,"save":{"includeText":true}},
                    "documentFormattingProvider":true,"workspace":{"workspaceFolders":{"supported":true}}});
                if self.mode.contains("tsserver") {
                    capabilities["executeCommandProvider"] = json!({"commands":["typescript.tsserverRequest"]});
                }
                if self.mode.starts_with("pull") {
                    capabilities["diagnosticProvider"] =
                        json!({"identifier":"fixture","interFileDependencies":false,"workspaceDiagnostics":false});
                }
                self.send(json!({"jsonrpc":"2.0","id":id,"result":{"capabilities":capabilities,"serverInfo":{"name":"ara-real-child-fixture","version":"1"}}})).await?;
            }
            "workspace/executeCommand" if self.mode.contains("tsserver") => {
                assert_eq!(params["command"], "typescript.tsserverRequest");
                assert_eq!(params["arguments"], json!(["reloadProjects", {}, {"executionTarget":0,"expectsResult":true,"isAsync":false,"lowPriority":false}]));
                self.send(json!({"jsonrpc":"2.0","id":id,"result":{"success":!self.mode.contains("reject")}})).await?;
            }
            "textDocument/didOpen" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap().to_owned();
                let document = Document {
                    text: params["textDocument"]["text"].as_str().unwrap_or_default().into(),
                    version: params["textDocument"]["version"].as_i64().unwrap_or(1),
                };
                if self.mode.contains("tsserver") {
                    let path = reqwest::Url::parse(&uri).unwrap().to_file_path().unwrap();
                    self.log(&json!({"event":"disk-at-open","uri":uri,"content":std::fs::read_to_string(path).ok()}));
                }
                self.documents.lock().await.insert(uri, document);
            }
            "textDocument/didChange" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap().to_owned();
                let mut documents = self.documents.lock().await;
                let document = documents.entry(uri).or_insert_with(|| Document { text: String::new(), version: 0 });
                for change in params["contentChanges"].as_array().unwrap() {
                    let text = change["text"].as_str().unwrap_or_default();
                    if change.get("range").is_none() {
                        document.text = text.into();
                    } else {
                        let start = position(&document.text, &change["range"]["start"]);
                        let end = position(&document.text, &change["range"]["end"]);
                        document.text.replace_range(start..end, text);
                    }
                }
                document.version = params["textDocument"]["version"].as_i64().unwrap_or(document.version + 1);
            }
            "textDocument/didSave" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap().to_owned();
                let document = self.documents.lock().await.get(&uri).cloned();
                if let Some(mut document) = document.filter(|_| self.mode != "pull-error") {
                    if let Some(text) = params["text"].as_str() {
                        document.text = text.into();
                    }
                    let server = self.clone();
                    tokio::spawn(async move {
                        let _ = server.publish(uri, document).await;
                    });
                }
            }
            "textDocument/didClose" => {
                if let Some(uri) = params["textDocument"]["uri"].as_str() {
                    self.documents.lock().await.remove(uri);
                }
            }
            "textDocument/formatting" => {
                if self.mode == "exit-format" {
                    std::process::exit(23);
                }
                let uri = params["textDocument"]["uri"].as_str().unwrap();
                let document = self.documents.lock().await.get(uri).cloned().unwrap();
                let server = self.clone();
                tokio::spawn(async move {
                    if server.mode == "format-hang" {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                    }
                    if document.text.contains("HANG_FORMAT") {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                    let formatted = document.text.replace("UNFORMATTED", "FORMATTED");
                    let edits = if formatted == document.text {
                        json!([])
                    } else {
                        json!([{"range":{"start":{"line":0,"character":0},"end":end_position(&document.text)},"newText":formatted}])
                    };
                    let _ = server.send(json!({"jsonrpc":"2.0","id":id,"result":edits})).await;
                });
            }
            "textDocument/diagnostic" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap();
                let document = self.documents.lock().await.get(uri).cloned().unwrap();
                let server = self.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(delay(&document.text)).await;
                    let result = if server.mode == "pull-error" {
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32603,"message":"fixture pull report unknown"}})
                    } else {
                        json!({"jsonrpc":"2.0","id":id,"result":{"kind":"full","items":diagnostics(&document.text)}})
                    };
                    let _ = server.send(result).await;
                });
            }
            "shutdown" => {
                self.send(json!({"jsonrpc":"2.0","id":id,"result":null})).await?;
            }
            "exit" => return Ok(false),
            "initialized" => {
                self.send(json!({"jsonrpc":"2.0","id":"fixture-progress","method":"window/workDoneProgress/create","params":{"token":"fixture-load"}})).await?;
                self.send(json!({"jsonrpc":"2.0","method":"$/progress","params":{"token":"fixture-load","value":{"kind":"begin","title":"fixture load"}}})).await?;
                self.send(json!({"jsonrpc":"2.0","method":"$/progress","params":{"token":"fixture-load","value":{"kind":"end"}}})).await?;
                if self.mode == "mux-state" {
                    self.send(json!({"jsonrpc":"2.0","id":"fixture-config","method":"workspace/configuration","params":{"items":[{"section":"fixture"}]}})).await?;
                    self.send(json!({"jsonrpc":"2.0","id":"fixture-register","method":"client/registerCapability","params":{"registrations":[{"id":"fixture-watch","method":"workspace/didChangeWatchedFiles","registerOptions":{"watchers":[{"globPattern":"**/*.fixture"}]}}]}})).await?;
                    self.send(json!({"jsonrpc":"2.0","method":"$/progress","params":{"token":"fixture-unfinished","value":{"kind":"begin","title":"unfinished fixture work"}}})).await?;
                }
            }
            "$/cancelRequest" | "workspace/didChangeConfiguration" | "workspace/didChangeWatchedFiles" => {}
            _ if id.is_some() => {
                self.send(
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"fixture method not found"}}),
                )
                .await?;
            }
            _ => {}
        }
        Ok(true)
    }
}
fn delay(text: &str) -> Duration {
    if text.contains("VERY_SLOW") {
        Duration::from_secs(2)
    } else if text.contains("SLOW") {
        Duration::from_millis(900)
    } else {
        Duration::from_millis(10)
    }
}
fn diagnostics(text: &str) -> Vec<Value> {
    text.lines().enumerate().filter_map(|(line, text)| {
        let (severity,message) = if text.contains("ERROR") { (1,"fixture issue") }
            else if text.contains("WARNING") { (2,"fixture warning") } else { return None; };
        Some(json!({"range":{"start":{"line":line,"character":0},"end":{"line":line,"character":1}},"severity":severity,"source":"fake-lsp","code":"FIXTURE","message":message}))
    }).collect()
}
fn end_position(text: &str) -> Value {
    let lines = text.split('\n').collect::<Vec<_>>();
    json!({"line":lines.len()-1,"character":lines.last().unwrap_or(&"").encode_utf16().count()})
}
fn position(text: &str, position: &Value) -> usize {
    let line = position["line"].as_u64().unwrap() as usize;
    let character = position["character"].as_u64().unwrap() as usize;
    let mut offset = 0;
    let mut lines = text.split_inclusive('\n');
    for _ in 0..line {
        offset += lines.next().map(str::len).unwrap_or(0);
    }
    let current = lines.next().unwrap_or_default();
    let mut units = 0;
    for (index, ch) in current.char_indices() {
        if units >= character {
            return offset + index;
        }
        units += ch.len_utf16();
    }
    offset + current.len()
}

fn vault_event(event: &str, argv: &[String]) -> io::Result<()> {
    use std::io::Write as _;
    if let Some(path) = std::env::var_os("ARA_FIXTURE_VAULT_LOG") {
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        let line = format!("{}\n", json!({"pid":std::process::id(),"event":event,"argv":argv}));
        file.write_all(line.as_bytes())?;
        file.flush()?;
    }
    Ok(())
}

async fn obsidian_fixture() -> io::Result<()> {
    let argv = std::env::args().skip(1).collect::<Vec<_>>();
    vault_event("spawn", &argv)?;
    let mode = argv
        .iter()
        .filter_map(|arg| arg.strip_prefix("query="))
        .find(|mode| matches!(*mode, "error-zero" | "error-stderr" | "exit-nonzero" | "hang"))
        .map(str::to_owned)
        .or_else(|| std::env::var("ARA_FIXTURE_VAULT_MODE").ok())
        .unwrap_or_default();
    let (stdout, stderr, exit) = match mode.as_str() {
        "error-zero" => ("Error: controlled zero\n".to_owned(), String::new(), 0),
        "error-stderr" => (String::new(), "Error: controlled stderr\n".to_owned(), 0),
        "exit-nonzero" => (String::new(), "controlled nonzero\n".to_owned(), 23),
        "hang" => {
            // A killed child has only its spawn receipt; no invented exit.
            tokio::time::sleep(Duration::from_secs(90)).await;
            (String::new(), String::new(), 0)
        }
        _ => {
            let command =
                if argv.first().is_some_and(|arg| arg.starts_with("vault=")) { &argv[1..] } else { &argv[..] };
            let output = if argv == ["vaults", "verbose"] {
                std::env::var("ARA_FIXTURE_VAULT_DIRECTORY").unwrap_or_else(|_| {
                    format!(
                        "Research Vault\t{}\nignored\n",
                        std::env::var("ARA_FIXTURE_VAULT_ROOT").unwrap_or_default()
                    )
                })
            } else if argv == ["vault", "info", "path"] {
                let active = std::env::var("ARA_FIXTURE_VAULT_ACTIVE")
                    .or_else(|_| std::env::var("ARA_FIXTURE_VAULT_ROOT"))
                    .unwrap_or_default();
                format!("path\t{active}\n")
            } else if command == ["vault", "info"] {
                "fixture vault info\n".to_owned()
            } else {
                format!("{}\n", json!({"argv":argv,"pid":std::process::id()}))
            };
            (output, String::new(), 0)
        }
    };
    let mut output = tokio::io::stdout();
    output.write_all(stdout.as_bytes()).await?;
    output.flush().await?;
    let mut error = tokio::io::stderr();
    error.write_all(stderr.as_bytes()).await?;
    error.flush().await?;
    vault_event("exit", &argv)?;
    if exit != 0 {
        std::process::exit(exit);
    }
    Ok(())
}

#[tokio::main]
async fn main() -> io::Result<()> {
    if std::env::current_exe()?.file_stem().and_then(|name| name.to_str()) == Some("obsidian") {
        return obsidian_fixture().await;
    }
    let mut mode = "push".to_owned();
    let mut log = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--mode" => mode = args.next().unwrap(),
            "--log" => log = args.next().map(PathBuf::from),
            _ => return Err(io::Error::new(io::ErrorKind::InvalidInput, "unknown fixture argument")),
        }
    }
    let fixture = Fixture {
        output: Arc::new(Mutex::new(tokio::io::stdout())),
        documents: Arc::new(Mutex::new(HashMap::new())),
        mode,
        log,
    };
    fixture.log(&json!({"event":"spawn"}));
    let mut input = tokio::io::BufReader::new(tokio::io::stdin());
    loop {
        let mut length = None;
        loop {
            let mut line = String::new();
            if input.read_line(&mut line).await? == 0 {
                fixture.log(&json!({"event":"eof"}));
                return Ok(());
            }
            if line == "\r\n" || line == "\n" {
                break;
            }
            if let Some(value) = line.trim().strip_prefix("Content-Length:") {
                length = value.trim().parse::<usize>().ok();
            }
        }
        let length = length
            .filter(|length| *length <= 16 * 1024 * 1024)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "fixture invalid LSP frame"))?;
        let mut bytes = vec![0; length];
        input.read_exact(&mut bytes).await?;
        let message = serde_json::from_slice(&bytes)?;
        if !fixture.handle(message).await? {
            fixture.log(&json!({"event":"exit"}));
            break;
        }
    }
    Ok(())
}
