//! Standalone stdio MCP fixture. Run by CLI integration tests as a real child.

use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::PathBuf;

fn send(value: Value) {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{value}").unwrap();
    out.flush().unwrap();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.windows(2).find(|pair| pair[0] == "--mode").map(|pair| pair[1].as_str()).unwrap_or("normal");
    let record = args.windows(2).find(|pair| pair[0] == "--record").map(|pair| PathBuf::from(&pair[1]));
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    while let Some(line) = lines.next() {
        let line = line.unwrap();
        let request: Value = serde_json::from_str(&line).unwrap();
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        match request.get("method").and_then(Value::as_str) {
            Some("initialize") => {
                if let Some(path) = &record {
                    std::fs::write(path, json!({"initialize":request["params"],"ambient_key_present":std::env::var_os("ARA_API_KEY").is_some(),"pid":std::process::id()}).to_string()).unwrap();
                }
                send(
                    json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"fake-mcp","version":"1"}}}),
                );
            }
            Some("notifications/initialized") if mode == "ask-roots" => {
                send(json!({"jsonrpc":"2.0","id":400,"method":"roots/list","params":{}}));
            }
            Some("tools/list") => {
                if mode == "malformed-frame" || mode == "truncated-frame" {
                    let mut out = std::io::stdout().lock();
                    if mode == "malformed-frame" {
                        out.write_all(b"{broken\n").unwrap();
                    } else {
                        out.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":").unwrap();
                    }
                    out.flush().unwrap();
                    std::process::exit(1);
                }
                if mode == "catalog-cycle" {
                    send(json!({"jsonrpc":"2.0","id":id,"result":{"tools":[],"nextCursor":"again"}}));
                    continue;
                }
                let names = if mode == "collision" {
                    vec!["foo-bar".to_owned(), "foo_bar".to_owned()]
                } else if mode == "catalog-overflow" {
                    (0..65).map(|n| format!("tool_{n}")).collect()
                } else {
                    vec!["echo".to_owned()]
                };
                let tools: Vec<Value> = names.iter().map(|name| json!({"name":name,"description":"Echo text","inputSchema":{"type":"object","properties":{"text":{"type":"string"},"marker":{"type":"string"}}}})).collect();
                send(json!({"jsonrpc":"2.0","id":id,"result":{"tools":tools}}));
                if mode == "ping-after-list" {
                    send(json!({"jsonrpc":"2.0","id":"idle-ping","method":"ping"}));
                    let pong: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
                    if let Some(path) = &record {
                        std::fs::write(path.with_extension("pong"), pong.to_string()).unwrap();
                    }
                }
                if mode == "linger-after-list" {
                    std::thread::sleep(std::time::Duration::from_secs(120));
                }
            }
            Some("tools/call") => {
                if mode == "ping-during-call" {
                    send(json!({"jsonrpc":"2.0","id":"call-ping","method":"ping"}));
                    let pong: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
                    if let Some(path) = &record {
                        std::fs::write(path.with_extension("pong"), pong.to_string()).unwrap();
                    }
                    if pong["id"] != "call-ping" || pong["result"] != json!({}) {
                        std::process::exit(2);
                    }
                }
                if let Some(path) = request["params"]["arguments"]["marker"].as_str() {
                    std::fs::write(path, "dispatched").unwrap();
                }
                if let Some(path) = &record {
                    let prior: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
                    std::fs::write(path, json!({"initialize":prior["initialize"],"ambient_key_present":prior["ambient_key_present"],"root_denied":prior["root_denied"],"call":request["params"]}).to_string()).unwrap();
                }
                if mode == "hang" {
                    std::thread::sleep(std::time::Duration::from_secs(120));
                } else if mode == "exit" {
                    std::process::exit(1);
                } else if mode == "large-jsonrpc-error" {
                    send(json!({"jsonrpc":"2.0","id":id,"error":{"code":777,"message":"x".repeat(900_000)}}));
                } else if mode == "large-result" {
                    send(
                        json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":"x".repeat(300_000)}]}}),
                    );
                } else if mode == "structured" {
                    send(json!({"jsonrpc":"2.0","id":id,"result":{
                        "content":[{"type":"resource","resource":{"uri":"file:///report.txt","text":"report body"}}],
                        "structuredContent":{"status":"ok","count":2}
                    }}));
                } else if mode == "structured-only" {
                    send(json!({"jsonrpc":"2.0","id":id,"result":{"content":[],"structuredContent":{"status":"ok"}}}));
                } else {
                    send(
                        json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":format!("echo: {}",request["params"]["arguments"]["text"].as_str().unwrap_or(""))}],"isError":false}}),
                    );
                }
            }
            None if id == json!(400) => {
                if let Some(path) = &record {
                    let prior: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
                    std::fs::write(path, json!({"initialize":prior["initialize"],"ambient_key_present":prior["ambient_key_present"],"root_denied":request["error"]["code"] == -32601}).to_string()).unwrap();
                }
            }
            _ => {}
        }
    }
}
