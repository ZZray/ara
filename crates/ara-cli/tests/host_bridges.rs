//! Real ara RPC/ACP children + controlled HTTP provider. Fixed OMP 596f2da:
//! host-tools.ts, host-uris.ts, rpc-mode.ts and edit-acp-bridge.test.ts.
//! A shared case deadline bounds all waits, including shutdown and reopen.

use ara_testkit::chunks::{done, finish, text, tool_call};
use ara_testkit::{FakeUpstream, Script};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_ara");
const CASE_BOUND: Duration = Duration::from_secs(8);

struct Env {
    home: tempfile::TempDir,
    work: tempfile::TempDir,
    sessions: PathBuf,
    deadline: Instant,
}

impl Env {
    fn new() -> Self {
        let home = tempfile::Builder::new().prefix("ara-host-home-").tempdir().unwrap();
        let work = tempfile::Builder::new().prefix("ara-host-work-").tempdir().unwrap();
        std::fs::create_dir(work.path().join(".git")).unwrap();
        let sessions = home.path().join("sessions");
        Self { home, work, sessions, deadline: Instant::now() + CASE_BOUND }
    }

    fn command(&self, upstream: &FakeUpstream, args: &[&str]) -> Command {
        self.mode_command(upstream, "rpc", args)
    }

    fn mode_command(&self, upstream: &FakeUpstream, mode: &str, args: &[&str]) -> Command {
        let mut command = Command::new(BIN);
        for key in [
            "OPENROUTER_API_KEY",
            "ANTHROPIC_API_KEY",
            "ARA_API_KEY",
            "ARA_TEST_API_KEY",
            "ARA_MODEL",
            "ARA_BASE_URL",
            "ARA_TEST_BASE_URL",
            "OPENROUTER_BASE_URL",
            "ARA_TEST_MODEL_ID",
            "CLAUDE_CONFIG_DIR",
            "COPILOT_HOME",
            "COPILOT_CUSTOM_INSTRUCTIONS_DIRS",
            "WSL_DISTRO_NAME",
            "WSL_INTEROP",
        ] {
            command.env_remove(key);
        }
        command
            .env("HOME", self.home.path())
            .env("ARA_HOME", self.home.path())
            .env("ARA_API_KEY", "sk-host-bridge-fixture-only")
            .args(["--mode", mode, "--model", "fake-model", "--base-url", &upstream.base_url(), "--cwd"])
            .arg(self.work.path())
            .arg("--session-dir")
            .arg(&self.sessions)
            .args(["--compact-threshold", "0"])
            .args(args);
        command
    }
}

struct RpcChild {
    child: Child,
    stdin: Option<ChildStdin>,
    frames: mpsc::Receiver<Result<Value, String>>,
    seen: Vec<Value>,
    stderr: Arc<Mutex<String>>,
    readers: Vec<JoinHandle<()>>,
    deadline: Instant,
}

impl RpcChild {
    fn spawn(env: &Env, upstream: &FakeUpstream, args: &[&str]) -> Self {
        Self::spawn_command(env, env.command(upstream, args))
    }

    fn spawn_acp(env: &Env, upstream: &FakeUpstream, args: &[&str]) -> Self {
        Self::spawn_command(env, env.mode_command(upstream, "acp", args))
    }

    fn spawn_command(env: &Env, mut command: Command) -> Self {
        let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let mut stderr_pipe = child.stderr.take().unwrap();
        let (tx, frames) = mpsc::channel();
        let output_reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let frame = line
                    .map_err(|error| error.to_string())
                    .and_then(|line| serde_json::from_str(&line).map_err(|error| format!("{error}: {line}")));
                if tx.send(frame).is_err() {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(String::new()));
        let captured = stderr.clone();
        let error_reader = std::thread::spawn(move || {
            let mut bytes = [0; 4096];
            while let Ok(length) = stderr_pipe.read(&mut bytes) {
                if length == 0 {
                    break;
                }
                captured.lock().unwrap().push_str(&String::from_utf8_lossy(&bytes[..length]));
            }
        });
        Self {
            child,
            stdin,
            frames,
            seen: Vec::new(),
            stderr,
            readers: vec![output_reader, error_reader],
            deadline: env.deadline,
        }
    }

    fn send(&mut self, frame: Value) {
        self.send_many(&[frame]);
    }

    fn send_many(&mut self, frames: &[Value]) {
        let stdin = self.stdin.as_mut().unwrap();
        for frame in frames {
            writeln!(stdin, "{frame}").unwrap();
        }
        stdin.flush().unwrap();
    }

    fn next(&mut self) -> Value {
        match self.frames.recv_timeout(self.deadline.saturating_duration_since(Instant::now())) {
            Ok(Ok(frame)) => {
                self.seen.push(frame.clone());
                frame
            }
            result => {
                panic!("RPC wait failed: {result:?}; stderr: {}; frames: {:?}", self.stderr.lock().unwrap(), self.seen)
            }
        }
    }

    fn until(&mut self, matches: impl Fn(&Value) -> bool) -> Value {
        loop {
            let frame = self.next();
            if matches(&frame) {
                return frame;
            }
        }
    }

    fn response(&mut self, id: &str) -> Value {
        self.until(|frame| frame["type"] == "response" && frame["id"] == id)
    }

    fn success(&mut self, id: &str) -> Value {
        let response = self.response(id);
        assert_eq!(response["success"], true, "{response}");
        response["data"].clone()
    }

    fn error(&mut self, id: &str) -> Value {
        let response = self.response(id);
        assert_eq!(response["success"], false, "{response}");
        response
    }

    fn ready(&mut self) {
        assert_eq!(self.next()["type"], "ready");
        let metadata = self.next();
        assert_eq!(metadata["type"], "available_commands_update");
        assert_eq!(metadata["commands"], json!([]));
    }

    fn state(&mut self, id: &str) -> Value {
        self.send(json!({"id":id,"type":"get_state"}));
        self.success(id)
    }

    fn prompt(&mut self, id: &str) {
        self.send(json!({"id":id,"type":"prompt","message":format!("bridge task {id}")}));
        self.success(id);
    }

    fn finish(&mut self) {
        self.join_after_eof(0);
        assert_eq!(self.seen.iter().filter(|frame| frame["type"] == "session_shutdown").count(), 1);
    }

    fn join_after_eof(&mut self, expected_exit: i32) {
        self.stdin.take();
        loop {
            match self.frames.recv_timeout(self.deadline.saturating_duration_since(Instant::now())) {
                Ok(Ok(frame)) => self.seen.push(frame),
                Ok(Err(error)) => panic!("{error}"),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(error) => panic!("RPC EOF did not settle: {error}; {}", self.stderr.lock().unwrap()),
            }
        }
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < self.deadline, "RPC child did not exit");
            std::thread::sleep(Duration::from_millis(5));
        };
        for reader in self.readers.drain(..) {
            reader.join().unwrap();
        }
        assert_eq!(status.code(), Some(expected_exit), "{}", self.stderr.lock().unwrap());
    }

    fn acp_send(&mut self, id: &str, method: &str, params: Value) {
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
    }

    fn acp_response(&mut self, id: &str) -> Value {
        // Concurrent cancel and prompt responses may settle in either order.
        // A response consumed while awaiting the other ID still belongs to it.
        let frame = match self.seen.iter().find(|frame| frame["id"] == id && frame.get("method").is_none()) {
            Some(frame) => frame.clone(),
            None => self.until(|frame| frame["id"] == id && frame.get("method").is_none()),
        };
        assert_eq!(frame["jsonrpc"], "2.0", "ACP stdout is protocol NDJSON: {frame}");
        frame
    }

    fn acp_success(&mut self, id: &str) -> Value {
        let frame = self.acp_response(id);
        assert!(frame.get("error").is_none(), "ACP error: {frame}; {}", self.stderr.lock().unwrap());
        frame.get("result").expect("JSON-RPC result").clone()
    }

    fn acp_request(&mut self, id: &str, method: &str, params: Value) -> Value {
        self.acp_send(id, method, params);
        self.acp_success(id)
    }

    fn acp_initialize(&mut self, capabilities: Value) {
        let result = self.acp_request(
            "acp-init",
            "initialize",
            json!({
                "protocolVersion":1,"clientInfo":{"name":"ara-host-bridges-peer","version":"1"},
                "clientCapabilities":capabilities,
            }),
        );
        assert_eq!(result["protocolVersion"], 1);
    }

    fn acp_new(&mut self, cwd: &Path) -> Value {
        self.acp_request("acp-new", "session/new", json!({"cwd":cwd,"mcpServers":[]}))
    }

    fn acp_prompt(&mut self, id: &str, session: &str) {
        self.acp_send(
            id,
            "session/prompt",
            json!({"sessionId":session,
            "prompt":[{"type":"text","text":format!("Real ACP bridge task {id}")}]}),
        );
    }

    fn acp_result(&self, call_id: &str) -> Value {
        let update = self
            .seen
            .iter()
            .find(|frame| {
                frame["method"] == "session/update"
                    && frame["params"]["update"]["sessionUpdate"] == "tool_call_update"
                    && frame["params"]["update"]["toolCallId"] == call_id
                    && matches!(frame["params"]["update"]["status"].as_str(), Some("completed" | "failed"))
            })
            .unwrap_or_else(|| panic!("Missing typed ACP terminal receipt for {call_id}: {:?}", self.seen));
        update["params"]["update"]["rawOutput"].clone()
    }

    fn peer_request(&mut self, method: &str, cursor: &mut usize) -> Value {
        loop {
            if let Some((index, frame)) = self
                .seen
                .iter()
                .enumerate()
                .skip(*cursor)
                .find(|(_, frame)| frame["method"] == method && frame.get("id").is_some())
            {
                *cursor = index + 1;
                return frame.clone();
            }
            self.next();
        }
    }

    fn peer_write(&mut self, session: &str, target: &Path, requested: &str, formatted: bool) -> Value {
        let frame = self.until(|frame| frame["method"] == "fs/write_text_file");
        assert_eq!(frame["jsonrpc"], "2.0");
        assert!(frame["id"].is_number() || frame["id"].is_string());
        let params = &frame["params"];
        assert_eq!(params["sessionId"], session);
        let actual_path = PathBuf::from(params["path"].as_str().unwrap());
        assert!(actual_path.is_absolute());
        assert_eq!(
            ara_tools::internal_urls::search_path_string(&actual_path),
            ara_tools::internal_urls::search_path_string(target)
        );
        assert_eq!(params["content"], requested);
        let persisted = if formatted {
            requested
                .split_inclusive('\n')
                .map(|line| line.strip_prefix("    ").map_or_else(|| line.to_owned(), |tail| format!("\t{tail}")))
                .collect::<String>()
        } else {
            requested.to_owned()
        };
        std::fs::write(&actual_path, persisted).unwrap();
        self.send(json!({"jsonrpc":"2.0","id":frame["id"],"result":{}}));
        frame
    }
}

impl Drop for RpcChild {
    fn drop(&mut self) {
        self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

async fn upstream(responses: Vec<Value>) -> FakeUpstream {
    FakeUpstream::start(serde_json::from_value::<Script>(json!({"responses":responses})).unwrap(), None).await.unwrap()
}

fn calls(calls: &[(&str, &str, Value)]) -> Value {
    let mut events = calls
        .iter()
        .enumerate()
        .map(|(index, (id, name, args))| tool_call(index as u64, id, name, &args.to_string()))
        .collect::<Vec<_>>();
    events.extend([finish("tool_calls"), done()]);
    json!({"events":events})
}

fn answer(message: &str) -> Value {
    json!({"events":[text(message),finish("stop"),done()]})
}

fn definition(name: &str, version: &str, hidden: bool) -> Value {
    json!({"name":name,"label":format!("Host {version}"),"description":format!("Host version {version}"),
        "hidden":hidden,"parameters":{"type":"object","properties":{"version":{"const":version}},
            "required":["version"],"additionalProperties":false}})
}

fn result(id: &Value, text: &str, version: &str) -> Value {
    json!({"type":"host_tool_result","id":id,"result":{"content":[{"type":"text","text":text}],"details":{"adapter":version}}})
}

fn text_content(value: &Value) -> String {
    value["content"].as_str().map(str::to_owned).unwrap_or_else(|| {
        value["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
    })
}

fn journal(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect()
}

fn receipt<'a>(entries: &'a [Value], id: &str) -> &'a Value {
    let matches = entries
        .iter()
        .filter(|entry| entry["message"]["role"] == "toolResult" && entry["message"]["toolCallId"] == id)
        .collect::<Vec<_>>();
    assert_eq!(matches.len(), 1, "exactly one durable receipt for {id}: {entries:?}");
    &matches[0]["message"]
}

fn assert_native_unknown_settlement(child: &RpcChild, id: &str) {
    let failures = child
        .seen
        .iter()
        .filter(|frame| {
            frame["type"] == "response"
                && frame["id"] == id
                && frame["command"] == "prompt"
                && frame["success"] == false
        })
        .collect::<Vec<_>>();
    assert_eq!(failures.len(), 1, "one owned settlement failure after admission for {id}: {:?}", child.seen);
    let error = failures[0]["error"].as_str().expect("original prompt has its settlement error");
    assert!(
        error.contains("Native entered effects have unknown outcomes")
            && error.contains("outcome_unknown")
            && error.contains("effects may be unknown"),
        "{error}"
    );
    eprintln!("Native Unknown settlement: {}", failures[0]);
}

fn schema<'a>(request: &'a Value, name: &str) -> &'a Value {
    &request["body"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["function"]["name"] == name)
        .expect("advertised host schema")["function"]
}

fn assert_no_side_channel_ack(child: &RpcChild, ids: &[Value]) {
    assert!(
        !child.seen.iter().any(|frame| frame["type"] == "response" && ids.contains(&frame["id"])),
        "valid bridge replies/updates are consumed without command ACK: {:?}",
        child.seen
    );
}

// Complete vault observations belong to this existing Host family. The copied
// fixture is found through production PATH lookup; there is no test override
// in HostVault. All environment values belong only to each actual ara child.
#[cfg(feature = "test-fixture")]
mod vault_family {
    use super::*;
    const OBSIDIAN_FIXTURE: &str = env!("CARGO_BIN_EXE_ara-lsp-fixture");
    fn vault_env(enabled: bool) -> (Env, PathBuf, PathBuf, PathBuf) {
        let mut env = Env::new();
        env.deadline = Instant::now() + Duration::from_secs(75);
        let root = env.home.path().join("vault");
        let active = env.home.path().join("active-vault");
        std::fs::create_dir_all(root.join("Folder/Sub")).unwrap();
        std::fs::create_dir_all(&active).unwrap();
        std::fs::write(root.join("Folder/note.md"), "# Note\nold body\n").unwrap();
        std::fs::write(root.join("note.ts"), "sharedSymbol(1);\n").unwrap();
        std::fs::write(active.join("note.md"), "active body\n").unwrap();
        let bin = env.home.path().join("vault-bin");
        std::fs::create_dir(&bin).unwrap();
        let executable = bin.join(if cfg!(windows) { "obsidian.exe" } else { "obsidian" });
        std::fs::copy(OBSIDIAN_FIXTURE, &executable).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        vault_config(&env, enabled);
        (env, root, active, bin)
    }

    fn vault_config(env: &Env, enabled: bool) {
        let directory = env.home.path().join("agent");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("config.yml"), format!(
        "vault:\n  enabled: {enabled}\nlsp:\n  enabled: false\nedit:\n  enforceSeenLines: false\nplan:\n  enabled: true\n"
    )).unwrap();
    }

    fn vault_command(env: &Env, up: &FakeUpstream, root: &Path, active: &Path, bin: &Path, mode: &str) -> Command {
        vault_policy_command(env, up, root, active, bin, mode, "replace")
    }
    fn vault_policy_command(
        env: &Env,
        up: &FakeUpstream,
        root: &Path,
        active: &Path,
        bin: &Path,
        mode: &str,
        edit_mode: &str,
    ) -> Command {
        let mut command =
            env.mode_command(up, mode, &["--tools", "read,write,edit,ast_grep,ast_edit", "--edit-mode", edit_mode]);
        command
            .env("PATH", bin)
            .env("ARA_FIXTURE_VAULT_ROOT", root)
            .env("ARA_FIXTURE_VAULT_ACTIVE", active)
            .env("ARA_FIXTURE_VAULT_LOG", env.home.path().join("vault-processes.jsonl"))
            .env_remove("ARA_FIXTURE_VAULT_MODE")
            .env_remove("ARA_FIXTURE_VAULT_DIRECTORY");
        command
    }

    fn vault_processes(env: &Env) -> Vec<Value> {
        let path = env.home.path().join("vault-processes.jsonl");
        if !path.exists() {
            return Vec::new();
        }
        journal(&path)
    }

    fn vault_alive(pid: u32) -> bool {
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::{
                Foundation::CloseHandle,
                System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
            };
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return false;
            }
            let mut code = 0;
            let known = GetExitCodeProcess(process, &mut code) != 0;
            CloseHandle(process);
            known && code == 259
        }
        #[cfg(unix)]
        unsafe {
            libc::kill(pid as i32, 0) == 0
                || std::io::Error::last_os_error().kind() == std::io::ErrorKind::PermissionDenied
        }
    }

    fn vault_settled(env: &Env) {
        for process in vault_processes(env).iter().filter(|entry| entry["event"] == "spawn") {
            assert!(!vault_alive(process["pid"].as_u64().unwrap() as u32), "Obsidian child not joined: {process}");
        }
    }

    fn vault_prompt(request: &Value) -> String {
        request["body"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "system")
            .map(text_content)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn vault_complete_protocol_cached_edit_ast_and_session_snapshots_use_real_children() {
        // The original 18 CA-MISC IDs remain in their inventory. This complete
        // family observes all shapes, all 25 exact CLI argv, immutable/readable
        // resources, cached EDIT/AST supply and native Session lifecycle together.
        let ops: Vec<(&str, &str, Vec<&str>, &str)> = vec![
            (
                "outline",
                "vault://Research%20Vault/Note.md?op=outline",
                vec!["outline", "path=Note.md", "format=md"],
                "text/markdown",
            ),
            (
                "backlinks",
                "vault://Research%20Vault/Note.md?op=backlinks",
                vec!["backlinks", "path=Note.md", "counts", "format=tsv"],
                "text/plain",
            ),
            ("links", "vault://Research%20Vault/Note.md?op=links", vec!["links", "path=Note.md"], "text/plain"),
            (
                "file-tags",
                "vault://Research%20Vault/Note.md?op=tags",
                vec!["tags", "path=Note.md", "counts", "format=json"],
                "application/json",
            ),
            (
                "file-properties",
                "vault://Research%20Vault/Note.md?op=properties",
                vec!["properties", "path=Note.md", "format=yaml"],
                "text/markdown",
            ),
            (
                "file-tasks",
                "vault://Research%20Vault/Note.md?op=tasks",
                vec!["tasks", "path=Note.md", "verbose", "format=json"],
                "application/json",
            ),
            (
                "wordcount",
                "vault://Research%20Vault/Note.md?op=wordcount",
                vec!["wordcount", "path=Note.md"],
                "text/plain",
            ),
            ("history", "vault://Research%20Vault/Note.md?op=history", vec!["history", "path=Note.md"], "text/plain"),
            (
                "base",
                "vault://Research%20Vault/Note.md?op=base&view=Main",
                vec!["base:query", "path=Note.md", "view=Main", "format=md"],
                "text/markdown",
            ),
            (
                "search",
                "vault://Research%20Vault?op=search&q=plan&path=Folder&limit=5&case",
                vec!["search:context", "query=plan", "path=Folder", "limit=5", "case", "format=json"],
                "application/json",
            ),
            ("daily", "vault://Research%20Vault?op=daily", vec!["daily:read"], "text/markdown"),
            ("daily-path", "vault://Research%20Vault?op=daily-path", vec!["daily:path"], "text/plain"),
            (
                "vault-tags",
                "vault://Research%20Vault?op=tags",
                vec!["tags", "counts", "format=json"],
                "application/json",
            ),
            ("tag", "vault://Research%20Vault?op=tag&name=%23todo", vec!["tag", "name=#todo", "verbose"], "text/plain"),
            (
                "vault-tasks",
                "vault://Research%20Vault?op=tasks",
                vec!["tasks", "todo", "verbose", "format=json"],
                "application/json",
            ),
            ("orphans", "vault://Research%20Vault?op=orphans", vec!["orphans"], "text/plain"),
            (
                "unresolved",
                "vault://Research%20Vault?op=unresolved",
                vec!["unresolved", "counts", "verbose", "format=json"],
                "application/json",
            ),
            ("deadends", "vault://Research%20Vault?op=deadends", vec!["deadends"], "text/plain"),
            ("bases", "vault://Research%20Vault?op=bases", vec!["bases"], "text/plain"),
            (
                "bookmarks",
                "vault://Research%20Vault?op=bookmarks",
                vec!["bookmarks", "verbose", "format=json"],
                "application/json",
            ),
            ("recents", "vault://Research%20Vault?op=recents", vec!["recents"], "text/plain"),
            ("templates", "vault://Research%20Vault?op=templates", vec!["templates"], "text/plain"),
            (
                "aliases",
                "vault://Research%20Vault?op=aliases",
                vec!["aliases", "verbose", "format=json"],
                "application/json",
            ),
            (
                "vault-properties",
                "vault://Research%20Vault?op=properties",
                vec!["properties", "counts", "format=yaml"],
                "text/markdown",
            ),
            (
                "property",
                "vault://Research%20Vault?op=property&name=status&path=Note.md",
                vec!["property:read", "name=status", "path=Note.md"],
                "text/plain",
            ),
        ];
        let op_calls = ops.iter().map(|(id, url, _, _)| (*id, "read", json!({"path":url}))).collect::<Vec<_>>();
        let up = upstream(vec![
        calls(&[
            ("uncached-edit","edit",json!({"path":"vault://Research%20Vault/Folder/note.md","old_string":"old body","new_string":"too early"})),
            ("uncached-ast-grep","ast_grep",json!({"path":"vault://Research%20Vault/","pat":"sharedSymbol($A)"})),
            ("uncached-ast-edit","ast_edit",json!({"paths":["vault://Research%20Vault/"],"ops":[{"pat":"sharedSymbol($A)","out":"tooEarly($A)"}]})),
        ]), answer("Cold cached-path refusals inspected before discovery."),
        calls(&[("vault-list","read",json!({"path":"vault://"}))]),
        calls(&[
            ("named-info","read",json!({"path":"vault://Research%20Vault"})),
            ("active-info","read",json!({"path":"vault://_"})),
            ("folder","read",json!({"path":"vault://Research%20Vault/Folder/"})),
            ("folder-no-slash","read",json!({"path":"vault://Research%20Vault/Folder"})),
            ("plain-note","read",json!({"path":"vault://Research%20Vault/Folder/note.md"})),
            ("active-note","read",json!({"path":"vault://_/note.md"})),
        ]),
        calls(&op_calls),
        calls(&[("vault-write","write",json!({"path":"vault://Research%20Vault/New/scratch.json","content":"{\"created\":true}"}))]),
        calls(&[("vault-edit","edit",json!({"path":"vault://Research%20Vault/Folder/note.md","old_string":"old body","new_string":"edited body"}))]),
        calls(&[
            ("vault-ast","ast_grep",json!({"path":"vault://Research%20Vault/note.ts","pat":"sharedSymbol($A)"})),
            ("vault-ast-directory","ast_grep",json!({"path":"vault://Research%20Vault/","pat":"sharedSymbol($A)"})),
        ]),
        calls(&[("vault-ast-edit","ast_edit",json!({"paths":["vault://Research%20Vault/"],"ops":[{"pat":"sharedSymbol($A)","out":"cachedSymbol($A)"}]}))]),
        json!({"events":[{"sleep_ms":500},
            tool_call(0,"vault-ast-apply","write",&json!({"path":"xd://resolve","content":"apply the structural rewrite from the discovered cached vault"}).to_string()),
            finish("tool_calls"),done()]}),
        calls(&[
            ("write-readback","read",json!({"path":"vault://Research%20Vault/New/scratch.json"})),
            ("edit-readback","read",json!({"path":"vault://Research%20Vault/Folder/note.md"})),
            ("ast-readback","read",json!({"path":"vault://Research%20Vault/note.ts"})),
            ("active-search","read",json!({"path":"vault://_?op=search&q=active+plan&q=last+plan"})),
            ("empty-host-search","read",json!({"path":"vault://?op=search&q=plan"})),
        ]), answer("Full vault receipts inspected."),
        calls(&[
            ("disabled-read","read",json!({"path":"vault://Research%20Vault/Folder/note.md"})),
            ("disabled-write","write",json!({"path":"vault://Research%20Vault/blocked.md","content":"blocked"})),
            ("disabled-edit","edit",json!({"path":"vault://Research%20Vault/Folder/note.md","old_string":"edited body","new_string":"blocked"})),
            ("disabled-ast","ast_grep",json!({"path":"vault://Research%20Vault/note.ts","pat":"sharedSymbol($A)"})),
        ]), answer("Disabled snapshot inspected."),
        calls(&[("restored-read","read",json!({"path":"vault://_/note.md"}))]), answer("Cached root survived new Session."),
        calls(&[("override-read","read",json!({"path":"vault://Research%20Vault/Folder/note.md"}))]), answer("Explicit override inspected."),
        calls(&[("removed-read","read",json!({"path":"vault://Research%20Vault/Folder/note.md"}))]), answer("Removed route inspected."),
    ]).await;
        let (env, root, active, bin) = vault_env(true);
        let first_date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let mut child = RpcChild::spawn_command(&env, vault_command(&env, &up, &root, &active, &bin, "rpc"));
        child.ready();
        child.prompt("vault-cold");
        child.until(|frame| frame["type"] == "agent_end");
        let file = PathBuf::from(child.state("vault-file")["sessionFile"].as_str().unwrap());
        let cold = journal(&file);
        for id in ["uncached-edit", "uncached-ast-grep", "uncached-ast-edit"] {
            assert_eq!(receipt(&cold, id)["isError"], true, "cold cached-path refusal: {id}");
            assert!(text_content(receipt(&cold, id)).contains("cached vault root"), "{id}: {:?}", receipt(&cold, id));
        }
        assert!(vault_processes(&env).is_empty(), "cold native EDIT/AST cannot discover a root or spawn Obsidian");
        assert_eq!(std::fs::read_to_string(root.join("Folder/note.md")).unwrap(), "# Note\nold body\n");
        assert_eq!(std::fs::read_to_string(root.join("note.ts")).unwrap(), "sharedSymbol(1);\n");
        child.prompt("vault-complete");
        let preview =
            child.until(|frame| frame["type"] == "tool_execution_end" && frame["toolCallId"] == "vault-ast-edit");
        assert_eq!(preview["isError"], false, "{preview}");
        assert_eq!(preview["result"]["details"]["applied"], false);
        assert_eq!(preview["result"]["details"]["totalReplacements"], 1);
        assert_eq!(
            std::fs::read_to_string(root.join("note.ts")).unwrap(),
            "sharedSymbol(1);\n",
            "registered directory AST proposal must not modify bytes before resolution"
        );
        assert_eq!(
            vault_processes(&env).iter().filter(|entry| entry["event"] == "spawn").count(),
            ops.len() + 4,
            "cached file/directory AST does not rediscover a vault"
        );
        child.until(|frame| frame["type"] == "agent_end");
        let entries = journal(&file);
        for id in [
            "vault-list",
            "named-info",
            "active-info",
            "folder",
            "folder-no-slash",
            "plain-note",
            "active-note",
            "vault-write",
            "vault-edit",
            "vault-ast",
            "vault-ast-directory",
            "vault-ast-edit",
            "vault-ast-apply",
            "write-readback",
            "edit-readback",
            "ast-readback",
        ] {
            assert_eq!(receipt(&entries, id)["isError"], false, "{id}: {:?}", receipt(&entries, id));
        }
        assert!(text_content(receipt(&entries, "vault-list")).contains("[Research Vault](vault://Research%20Vault/)"));
        for id in ["folder", "folder-no-slash"] {
            let text = text_content(receipt(&entries, id));
            assert!(text.contains("[Sub/](vault://Research%20Vault/Folder/Sub/)"));
            assert!(text.contains("[note.md](vault://Research%20Vault/Folder/note.md)"));
        }
        assert_eq!(receipt(&entries, "plain-note")["details"]["contentType"], "text/markdown");
        assert_eq!(receipt(&entries, "write-readback")["details"]["contentType"], "application/json");
        assert!(text_content(receipt(&entries, "named-info")).contains("fixture vault info"));
        assert!(text_content(receipt(&entries, "active-note")).contains("active body"));
        assert!(text_content(receipt(&entries, "vault-ast")).contains("sharedSymbol"));
        assert!(text_content(receipt(&entries, "vault-ast-directory")).contains("sharedSymbol"));
        assert_eq!(receipt(&entries, "vault-ast-edit")["details"]["applied"], false);
        let applied = &receipt(&entries, "vault-ast-apply")["details"]["xdev"]["inner"];
        assert_eq!(applied["sourceToolName"], "ast_edit");
        assert_eq!(applied["sourceResultDetails"]["applied"], true);
        assert_eq!(applied["sourceResultDetails"]["totalReplacements"], 1);
        assert!(text_content(receipt(&entries, "ast-readback")).contains("cachedSymbol(1)"));
        assert_eq!(std::fs::read_to_string(root.join("note.ts")).unwrap(), "cachedSymbol(1);\n");
        assert_eq!(std::fs::read_to_string(root.join("Folder/note.md")).unwrap(), "# Note\nedited body\n");
        assert_eq!(std::fs::read_to_string(root.join("New/scratch.json")).unwrap(), "{\"created\":true}");
        let processes = vault_processes(&env);
        let spawned = processes.iter().filter(|entry| entry["event"] == "spawn").collect::<Vec<_>>();
        assert_eq!(
            spawned.len(),
            ops.len() + 6,
            "only original discovery/info/explicit CLI operations spawn; AST/readback use the cache"
        );
        for argv in [
            json!(["vaults", "verbose"]),
            json!(["vault", "info", "path"]),
            json!(["vault=Research Vault", "vault", "info"]),
            json!(["vault", "info"]),
        ] {
            assert_eq!(
                spawned.iter().filter(|entry| entry["argv"] == argv).count(),
                1,
                "cache dispatch {argv}: {spawned:?}"
            );
        }
        for (id, _, args, content_type) in &ops {
            let mut expected = vec!["vault=Research Vault"];
            expected.extend(args);
            assert_eq!(spawned.iter().filter(|entry| entry["argv"] == json!(expected)).count(), 1, "exact argv {id}");
            assert_eq!(receipt(&entries, id)["isError"], false, "{id}");
            assert_eq!(receipt(&entries, id)["details"]["contentType"], *content_type, "{id}");
        }
        for argv in [
            json!(["search:context", "query=last plan", "format=json"]),
            json!(["search:context", "query=plan", "format=json"]),
        ] {
            assert_eq!(spawned.iter().filter(|entry| entry["argv"] == argv).count(), 1, "active argv {argv}");
        }
        let before = spawned.len();
        vault_config(&env, false);
        child.send(json!({"id":"vault-disabled-new","type":"new_session"}));
        child.success("vault-disabled-new");
        child.prompt("vault-disabled");
        child.until(|frame| frame["type"] == "agent_end");
        let disabled_file = PathBuf::from(child.state("disabled-file")["sessionFile"].as_str().unwrap());
        assert_ne!(file, disabled_file);
        for id in ["disabled-read", "disabled-write", "disabled-edit", "disabled-ast"] {
            let disabled = journal(&disabled_file);
            assert_eq!(receipt(&disabled, id)["isError"], true, "{id}");
            assert!(text_content(receipt(&disabled, id)).contains("disabled"), "{id}: {:?}", receipt(&disabled, id));
        }
        assert!(!root.join("blocked.md").exists());
        std::fs::write(env.home.path().join("agent/config.yml"), "vault: {enabled: true}\nlsp: [\n").unwrap();
        child.send(json!({"id":"bad-vault-preparation","type":"new_session"}));
        child.error("bad-vault-preparation");
        assert_eq!(child.state("still-disabled")["sessionFile"], json!(disabled_file));
        vault_config(&env, true);
        child.send(json!({"id":"vault-restored-new","type":"new_session"}));
        child.success("vault-restored-new");
        child.prompt("vault-restored");
        child.until(|frame| frame["type"] == "agent_end");
        assert_eq!(
            vault_processes(&env).iter().filter(|entry| entry["event"] == "spawn").count(),
            before,
            "process cache survives Session replacement"
        );
        child.send(json!({"id":"override-vault","type":"set_host_uri_schemes","schemes":[{"scheme":"vault"}]}));
        child.success("override-vault");
        child.prompt("vault-override");
        let request = child.until(|frame| frame["type"] == "host_uri_request");
        assert_eq!(request["operation"], "read");
        child.send(json!({"type":"host_uri_result","id":request["id"],"content":"explicit vault owner"}));
        child.until(|frame| frame["type"] == "agent_end");
        child.send(json!({"id":"remove-vault","type":"set_host_uri_schemes","schemes":[]}));
        child.success("remove-vault");
        child.prompt("vault-removed");
        child.until(|frame| frame["type"] == "agent_end");
        let restored_file = PathBuf::from(child.state("restored-file")["sessionFile"].as_str().unwrap());
        child.finish();
        vault_settled(&env);
        let restored = journal(&restored_file);
        assert!(text_content(receipt(&restored, "override-read")).contains("explicit vault owner"));
        assert_eq!(receipt(&restored, "removed-read")["isError"], true);
        assert!(text_content(receipt(&restored, "removed-read")).contains("Unknown protocol"));
        assert_eq!(
            child.seen.iter().filter(|frame| frame["type"] == "host_uri_request").count(),
            1,
            "one explicit dispatch and no native bypass"
        );
        let requests = up.requests.lock().await;
        for (id, enabled) in
            [("vault-cold", true), ("vault-complete", true), ("vault-disabled", false), ("vault-restored", true)]
        {
            let submitted = format!("bridge task {id}");
            // Fixed date-cwd-reminder.ts injects the first user turn in each
            // Session. Match the exact source-backed prefix and original task;
            // two dates allow this bounded family to cross local midnight.
            let reminded = [first_date.clone(), chrono::Local::now().format("%Y-%m-%d").to_string()].map(|date| {
                format!(
                    "{}\n\n{submitted}",
                    ara_context::render_date_cwd_reminder(&date, &env.work.path().to_string_lossy().replace('\\', "/"))
                )
            });
            let request = requests
                .iter()
                .find(|request| {
                    request["body"]["messages"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .rev()
                        .find(|message| message["role"] == "user")
                        .is_some_and(|message| {
                            let content = text_content(message);
                            content == submitted || reminded.contains(&content)
                        })
                })
                .unwrap_or_else(|| {
                    let last_users = requests
                        .iter()
                        .filter_map(|request| {
                            request["body"]["messages"]
                                .as_array()?
                                .iter()
                                .rev()
                                .find(|message| message["role"] == "user")
                        })
                        .map(text_content)
                        .collect::<std::collections::BTreeSet<_>>();
                    panic!(
                        "request correlated with {id}: expected {submitted:?} or {reminded:?}; actual {last_users:?}"
                    )
                });
            let user = request["body"]["messages"]
                .as_array()
                .unwrap()
                .iter()
                .rev()
                .find(|message| message["role"] == "user")
                .unwrap();
            eprintln!("Vault correlated wire {id}: {:?}", text_content(user));
            assert_eq!(vault_prompt(request).contains("vault://"), enabled, "{id}: prepared prompt snapshot");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn vault_validation_errors_missing_binary_cancel_timeout_and_reopen_settle_real_children() {
        let (env, root, active, bin) = vault_env(true);
        let mut rejected: Vec<(&str, &str, Value)> = vec![
            ("literal-traversal", "read", json!({"path":"vault://Research%20Vault/../secret.md"})),
            ("encoded-traversal", "read", json!({"path":"vault://Research%20Vault/%2E%2E/secret.md"})),
            ("absolute-write", "write", json!({"path":"vault://Research%20Vault//absolute.md","content":"bad"})),
            ("invalid-encoding", "read", json!({"path":"vault://Research%20Vault/%ZZ.md"})),
            ("file-op-invalid", "read", json!({"path":"vault://Research%20Vault/Note.md?op=eval"})),
            ("vault-op-invalid", "read", json!({"path":"vault://Research%20Vault?op=eval"})),
            ("search-required", "read", json!({"path":"vault://Research%20Vault?op=search&q"})),
            ("base-required", "read", json!({"path":"vault://Research%20Vault/Note.md?op=base"})),
            ("property-required", "read", json!({"path":"vault://Research%20Vault?op=property&name=status"})),
            ("query-traversal", "read", json!({"path":"vault://Research%20Vault?op=search&q=x&path=../secret"})),
            ("operation-write", "write", json!({"path":"vault://Research%20Vault/Note.md?op=outline","content":"bad"})),
            ("directory-write", "write", json!({"path":"vault://Research%20Vault/Folder/","content":"bad"})),
            ("missing-file", "read", json!({"path":"vault://Research%20Vault/missing.md"})),
            ("unknown-vault", "read", json!({"path":"vault://unknown/note.md"})),
            ("error-zero", "read", json!({"path":"vault://_?op=search&q=error-zero"})),
            ("error-stderr", "read", json!({"path":"vault://_?op=search&q=error-stderr"})),
            ("exit-nonzero", "read", json!({"path":"vault://_?op=search&q=exit-nonzero"})),
        ];
        // Same fixed Unix symlink boundary as the original vault test. Windows
        // traversal/ancestor canonicalization is exercised above without symlinks.
        #[cfg(unix)]
        {
            let outside = env.home.path().join("outside");
            std::fs::create_dir(&outside).unwrap();
            std::fs::write(outside.join("secret.md"), "secret").unwrap();
            std::os::unix::fs::symlink(&outside, root.join("linked")).unwrap();
            rejected.extend([
                ("symlink-read", "read", json!({"path":"vault://Research%20Vault/linked/secret.md"})),
                ("symlink-write", "write", json!({"path":"vault://Research%20Vault/linked/new.md","content":"bad"})),
            ]);
        }
        let up = upstream(vec![
            calls(&rejected),
            answer("Validation failures inspected."),
            calls(&[("restart-note", "read", json!({"path":"vault://Research%20Vault/Folder/note.md"}))]),
            answer("Original journal and native vault restored."),
        ])
        .await;
        let mut child = RpcChild::spawn_command(&env, vault_command(&env, &up, &root, &active, &bin, "rpc"));
        child.ready();
        child.prompt("vault-failures");
        child.until(|frame| frame["type"] == "agent_end");
        let file = PathBuf::from(child.state("failure-file")["sessionFile"].as_str().unwrap());
        child.finish();
        vault_settled(&env);
        let entries = journal(&file);
        for (id, _, _) in &rejected {
            assert_eq!(receipt(&entries, id)["isError"], true, "{id}: {:?}", receipt(&entries, id));
        }
        for id in ["literal-traversal", "encoded-traversal", "query-traversal"] {
            assert!(text_content(receipt(&entries, id)).contains("traversal"), "{id}");
        }
        assert!(text_content(receipt(&entries, "absolute-write")).contains("Absolute paths"));
        assert!(text_content(receipt(&entries, "invalid-encoding")).contains("Invalid URL encoding"));
        assert!(text_content(receipt(&entries, "unknown-vault")).contains("Available: Research Vault"));
        assert!(text_content(receipt(&entries, "missing-file")).contains("Vault file not found"));
        for (id, detail) in [
            ("error-zero", "Error: controlled zero"),
            ("error-stderr", "Error: controlled stderr"),
            ("exit-nonzero", "controlled nonzero"),
        ] {
            assert!(text_content(receipt(&entries, id)).contains(detail), "{id}");
        }
        assert!(!root.join("absolute.md").exists());
        assert_eq!(std::fs::read_to_string(root.join("Folder/note.md")).unwrap(), "# Note\nold body\n");
        let first_processes = vault_processes(&env);
        assert_eq!(
            first_processes.iter().filter(|entry| entry["event"] == "spawn").count(),
            4,
            "one directory discovery + three failing CLI commands; validation does not spawn"
        );
        let mut restart_command = vault_command(&env, &up, &root, &active, &bin, "rpc");
        restart_command.arg("--resume").arg(&file);
        let mut restarted = RpcChild::spawn_command(&env, restart_command);
        restarted.ready();
        assert_eq!(restarted.state("restarted-state")["sessionFile"], json!(file));
        restarted.prompt("reopened-vault");
        restarted.until(|frame| frame["type"] == "agent_end");
        restarted.finish();
        vault_settled(&env);
        assert_eq!(receipt(&journal(&file), "restart-note")["isError"], false);
        assert_eq!(
            vault_processes(&env).iter().filter(|entry| entry["event"] == "spawn").count(),
            5,
            "new process discovers once and does not replay original failures"
        );

        // Each actual process has a new process-wide binary cache. The enabled
        // missing-binary case hides the advertisement and returns the fixed error.
        let (missing_env, missing_root, missing_active, missing_bin) = vault_env(true);
        std::fs::remove_file(missing_bin.join(if cfg!(windows) { "obsidian.exe" } else { "obsidian" })).unwrap();
        let missing_up = upstream(vec![
            calls(&[("missing-binary", "read", json!({"path":"vault://_?op=search&q=plan"}))]),
            answer("Missing binary inspected."),
        ])
        .await;
        let mut missing = RpcChild::spawn_command(
            &missing_env,
            vault_command(&missing_env, &missing_up, &missing_root, &missing_active, &missing_bin, "rpc"),
        );
        missing.ready();
        missing.prompt("missing-obsidian");
        missing.until(|frame| frame["type"] == "agent_end");
        let missing_file = PathBuf::from(missing.state("missing-file")["sessionFile"].as_str().unwrap());
        missing.finish();
        let missing_entries = journal(&missing_file);
        assert!(text_content(receipt(&missing_entries, "missing-binary")).contains("Checked PATH entry 'obsidian'"));
        assert!(!vault_prompt(&missing_up.requests.lock().await[0]).contains("vault://"));
        assert!(vault_processes(&missing_env).is_empty());

        let (hash_env, hash_root, hash_active, hash_bin) = vault_env(true);
        // Read must supply the real mutable file snapshot and seen lines. A
        // precomputed anchor alone is insufficient when enforceSeenLines is on.
        let settings = hash_env.home.path().join("agent/config.yml");
        let authored =
            std::fs::read_to_string(&settings).unwrap().replace("enforceSeenLines: false", "enforceSeenLines: true");
        std::fs::write(&settings, authored).unwrap();
        let tag = ara_edit::store::file_hash("# Note\nold body\n");
        let hash_up = upstream(vec![
            calls(&[("mutable-hash-read", "read", json!({"path":"vault://Research%20Vault/Folder/note.md"}))]),
            calls(&[(
                "mutable-hash-edit",
                "edit",
                json!({"input":format!("[vault://Research%20Vault/Folder/note.md#{tag}]\nPUT 2-2:\n+hash edited")}),
            )]),
            calls(&[("mutable-hash-readback", "read", json!({"path":"vault://Research%20Vault/Folder/note.md"}))]),
            answer("Mutable vault tag and seen-line receipt inspected."),
        ])
        .await;
        let mut hash_child = RpcChild::spawn_command(
            &hash_env,
            vault_policy_command(&hash_env, &hash_up, &hash_root, &hash_active, &hash_bin, "rpc", "hashline"),
        );
        hash_child.ready();
        hash_child.prompt("mutable-hashline-run");
        hash_child.until(|frame| frame["type"] == "agent_end");
        let hash_file = PathBuf::from(hash_child.state("hash-file")["sessionFile"].as_str().unwrap());
        hash_child.finish();
        vault_settled(&hash_env);
        let hash_entries = journal(&hash_file);
        let read = receipt(&hash_entries, "mutable-hash-read");
        assert_eq!(read["isError"], false);
        assert!(text_content(read).contains(&format!("#{tag}]")), "native file tag: {read}");
        assert_eq!(read["details"]["meta"]["source"]["value"], "vault://Research%20Vault/Folder/note.md");
        assert_eq!(
            receipt(&hash_entries, "mutable-hash-edit")["isError"],
            false,
            "{:?}",
            receipt(&hash_entries, "mutable-hash-edit")
        );
        assert!(text_content(receipt(&hash_entries, "mutable-hash-readback")).contains("hash edited"));
        assert_eq!(std::fs::read_to_string(hash_root.join("Folder/note.md")).unwrap(), "# Note\nhash edited\n");

        for cancel in [true, false] {
            let (mut env, root, active, bin) = vault_env(true);
            // The fixed production timeout is 30s; allow child kill, pipe join,
            // Agent settlement and final journal inspection in this same family.
            env.deadline = Instant::now() + Duration::from_secs(45);
            let up = upstream(vec![
                calls(&[("owned-hang", "read", json!({"path":"vault://_?op=search&q=hang"}))]),
                answer("Owned timeout receipt inspected."),
            ])
            .await;
            let mut child = RpcChild::spawn_command(&env, vault_command(&env, &up, &root, &active, &bin, "rpc"));
            child.ready();
            child.prompt(if cancel { "cancel-owned-child" } else { "timeout-owned-child" });
            let started = Instant::now();
            while !vault_processes(&env).iter().any(|entry| entry["event"] == "spawn") {
                assert!(Instant::now() < env.deadline, "Obsidian CLI never spawned");
                std::thread::sleep(Duration::from_millis(5));
            }
            if cancel {
                child.send(json!({"id":"vault-abort","type":"abort"}));
                child.success("vault-abort");
            }
            // Abort ACK may arrive after the terminal event. This is a fresh
            // child with exactly one Run; do not wait for a second agent_end.
            if !child.seen.iter().any(|frame| frame["type"] == "agent_end") {
                child.until(|frame| frame["type"] == "agent_end");
            }
            if !cancel {
                assert!(started.elapsed() >= Duration::from_secs(29), "must use production 30s timeout");
            }
            let file = PathBuf::from(child.state("hang-file")["sessionFile"].as_str().unwrap());
            child.finish();
            vault_settled(&env);
            let entries = journal(&file);
            assert_eq!(receipt(&entries, "owned-hang")["isError"], true);
            let error = text_content(receipt(&entries, "owned-hang"));
            assert!(
                if cancel {
                    error.to_lowercase().contains("cancel") || error.to_lowercase().contains("abort")
                } else {
                    error.contains("timed out after 30000ms")
                },
                "{error}"
            );
            let processes = vault_processes(&env);
            assert_eq!(processes.iter().filter(|entry| entry["event"] == "spawn").count(), 1);
            assert!(!processes.iter().any(|entry| entry["event"] == "exit"), "killed child never manufactures success");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn acp_vault_and_plan_uri_composition_preserves_typed_owner_and_cached_policy() {
        let (env, root, active, bin) = vault_env(true);
        let up = upstream(vec![
        calls(&[("acp-vault-read","read",json!({"path":"vault://Research%20Vault/Folder/note.md"}))]),
        calls(&[("acp-vault-write","write",json!({"path":"vault://Research%20Vault/acp.md","content":"acp native vault bytes\n"}))]),
        calls(&[("acp-vault-edit","edit",json!({"path":"vault://Research%20Vault/Folder/note.md","old_string":"old body","new_string":"acp edited"}))]),
        calls(&[("acp-vault-ast","ast_grep",json!({"path":"vault://Research%20Vault/note.ts","pat":"sharedSymbol($A)"}))]),
        answer("ACP native vault receipts inspected."),
        calls(&[("acp-plan-draft","write",json!({"path":"local://vault-plan.md","content":"# Vault plan\nInspect original receipt.\n"}))]),
        calls(&[("acp-plan-proposal","write",json!({"path":"xd://propose","content":"vault"}))]),
        answer("Typed plan owner survived native vault composition."),
    ]).await;
        let mut child = RpcChild::spawn_command(&env, vault_command(&env, &up, &root, &active, &bin, "acp"));
        child.acp_initialize(json!({}));
        let opened = child.acp_new(env.work.path());
        let id = opened["sessionId"].as_str().unwrap();
        child.acp_prompt("acp-vault-run", id);
        child.acp_success("acp-vault-run");
        assert_eq!(std::fs::read_to_string(root.join("acp.md")).unwrap(), "acp native vault bytes\n");
        assert_eq!(std::fs::read_to_string(root.join("Folder/note.md")).unwrap(), "# Note\nacp edited\n");
        let file = acp_session_file(&env, id);
        let entries = journal(&file);
        for call in ["acp-vault-read", "acp-vault-write", "acp-vault-edit", "acp-vault-ast"] {
            assert_eq!(receipt(&entries, call)["isError"], false, "{call}: {:?}", receipt(&entries, call));
            assert!(!child.acp_result(call).is_null(), "typed terminal result: {call}");
        }
        child.acp_request("acp-vault-plan-mode", "session/set_mode", json!({"sessionId":id,"modeId":"plan"}));
        child.acp_prompt("acp-vault-plan-run", id);
        child.acp_success("acp-vault-plan-run");
        let entries = journal(&file);
        assert_eq!(receipt(&entries, "acp-plan-proposal")["isError"], false);
        assert!(!child.acp_result("acp-plan-proposal").is_null());
        let mode = child.acp_request(
            "acp-vault-final-mode",
            "session/resume",
            json!({"sessionId":id,"cwd":env.work.path(),"mcpServers":[]}),
        );
        assert_eq!(mode["modes"]["currentModeId"], "default");
        child.join_after_eof(0);
        vault_settled(&env);
        assert!(
            !child.seen.iter().any(|frame| frame["method"] == "fs/write_text_file"),
            "vault is native filesystem, not client bridge"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn host_tools_replace_inside_run_keep_pending_adapter_hidden_survivor_and_native_receipts() {
    let up = upstream(vec![
        calls(&[("model-A", "custom", json!({"version":"A"}))]),
        calls(&[("model-B", "custom", json!({"version":"B"}))]),
        answer("Both host receipts accepted."),
    ])
    .await;
    let env = Env::new();
    let mut child = RpcChild::spawn(&env, &up, &["--tools", "read"]);
    child.ready();
    child.send(json!({"id":"register-A","type":"set_host_tools","tools":[definition(" custom ","A",false)]}));
    assert_eq!(child.success("register-A"), json!({"toolNames":["custom"]}));
    child.prompt("same-run");
    let first = child.until(|frame| frame["type"] == "host_tool_call");
    assert_eq!(first["toolCallId"], "model-A");
    assert_ne!(first["id"], first["toolCallId"]);
    assert_eq!(first["arguments"], json!({"version":"A"}));
    let file = PathBuf::from(child.state("pending-file")["sessionFile"].as_str().unwrap());
    child.send(json!({"id":"register-B","type":"set_host_tools","tools":[
        definition("custom","B",true),definition("new_hidden","H",true),definition("fresh","F",false)]}));
    assert_eq!(child.success("register-B")["toolNames"], json!(["custom", "new_hidden", "fresh"]));
    child.send(json!({"id":"duplicate","type":"set_host_tools","tools":[definition("custom","C",false),definition("custom","C",false)]}));
    assert!(child.error("duplicate")["error"].as_str().unwrap().contains("unique"));
    child.send(json!({"id":"collision","type":"set_host_tools","tools":[definition("read","C",false)]}));
    assert!(child.error("collision")["error"].as_str().unwrap().contains("conflicts"));
    let state = child.state("retained-B");
    let active = state["dumpTools"].as_array().unwrap();
    assert!(active.iter().any(|tool| tool["name"] == "read"));
    assert!(active.iter().any(|tool| tool["name"] == "fresh"));
    assert!(!active.iter().any(|tool| tool["name"] == "new_hidden"));
    assert_eq!(
        active.iter().find(|tool| tool["name"] == "custom").unwrap()["parameters"]["properties"]["version"]["const"],
        "B"
    );
    child.send(json!({"type":"host_tool_update","id":first["id"],"partialResult":{"content":[{"type":"text","text":"host progress A"}],"details":{"step":1}}}));
    let update = child.until(|frame| frame["type"] == "tool_execution_update" && frame["toolCallId"] == "model-A");
    assert_eq!(text_content(&update["partialResult"]), "host progress A");
    assert_eq!(update["partialResult"]["details"]["step"], 1);
    child.send(result(&first["id"], "receipt A", "A"));
    let second = child.until(|frame| frame["type"] == "host_tool_call" && frame["toolCallId"] == "model-B");
    assert_ne!(second["id"], first["id"]);
    assert_eq!(second["arguments"], json!({"version":"B"}));
    child.send(result(&first["id"], "stale A must be ignored", "stale"));
    assert_eq!(child.state("stale-barrier")["isStreaming"], true);
    assert!(!child.seen.iter().any(|frame| frame["type"] == "tool_execution_end" && frame["toolCallId"] == "model-B"));
    child.send(result(&second["id"], "receipt B", "B"));
    child.until(|frame| frame["type"] == "agent_end");
    child.finish();
    assert_no_side_channel_ack(&child, &[first["id"].clone(), second["id"].clone()]);
    let entries = journal(&file);
    assert_eq!(receipt(&entries, "model-A")["details"]["adapter"], "A");
    assert_eq!(receipt(&entries, "model-B")["details"]["adapter"], "B");
    assert!(entries.iter().any(|entry| text_content(&entry["message"]).contains("Both host receipts accepted.")));
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 3);
    for (request, version) in [(&requests[0], "A"), (&requests[1], "B")] {
        let parameters = &schema(request, "custom")["parameters"];
        assert_eq!(parameters["type"], "object");
        assert_eq!(parameters["required"], json!(["i", "version"]));
        assert_eq!(parameters["properties"]["version"]["type"], "string");
        assert_eq!(parameters["properties"]["version"]["enum"], json!([version]));
        assert!(parameters["properties"]["version"].get("const").is_none());
    }
    assert!(
        !requests[1]["body"]["tools"].as_array().unwrap().iter().any(|tool| tool["function"]["name"] == "new_hidden")
    );
    assert!(requests[1]["body"]["messages"].as_array().unwrap().iter().any(|message| message["role"] == "tool"
        && message["tool_call_id"] == "model-A"
        && text_content(message) == "receipt A"));
}

#[tokio::test(flavor = "multi_thread")]
async fn host_uri_builtin_read_write_preserve_content_type_strip_display_and_enforce_readonly() {
    let up = upstream(vec![
        calls(&[("db-read", "read", json!({"path":"db://table"}))]),
        calls(&[(
            "db-write",
            "write",
            json!({"path":"[db://table#ABCD]","content":"[db://table#ABCD]\n1:标题😀\n2:end"}),
        )]),
        calls(&[("readonly-write", "write", json!({"path":"ro://table","content":"denied"}))]),
        calls(&[("readonly-read", "read", json!({"path":"ro://table"}))]),
        answer("URI work finished."),
    ])
    .await;
    let env = Env::new();
    let mut child = RpcChild::spawn(&env, &up, &["--tools", "read,write,edit"]);
    child.ready();
    child.send(json!({"id":"schemes","type":"set_host_uri_schemes","schemes":[
        {"scheme":" DB ","writable":false},{"scheme":"ro","immutable":true},{"scheme":"db","writable":true,"immutable":true}]}));
    assert_eq!(child.success("schemes")["schemes"], json!(["db", "ro"]));
    child.send(json!({"id":"reserved-scheme","type":"set_host_uri_schemes","schemes":[{"scheme":"security"}]}));
    assert!(child.error("reserved-scheme")["error"].as_str().unwrap().contains("reserved"));
    child.prompt("uri-run");
    let read = child.until(|frame| frame["type"] == "host_uri_request");
    assert_eq!(read["operation"], "read");
    assert_eq!(read["url"], "db://table");
    assert!(read.get("content").is_none());
    child.send(json!({"type":"host_uri_result","id":read["id"],"content":"标题😀\nnext","contentType":"text/markdown","immutable":false,"notes":["host note"]}));
    let write = child.until(|frame| frame["type"] == "host_uri_request" && frame["operation"] == "write");
    assert_eq!(write["url"], "db://table");
    assert_eq!(write["content"], "标题😀\nend");
    child.send(json!({"type":"host_uri_result","id":write["id"],"content":"successful write payload is ignored"}));
    let readonly = child.until(|frame| frame["type"] == "host_uri_request" && frame["url"] == "ro://table");
    assert_eq!(readonly["operation"], "read");
    child.send(json!({"type":"host_uri_result","id":readonly["id"],"content":"readonly text"}));
    child.until(|frame| frame["type"] == "agent_end");
    let file = PathBuf::from(child.state("uri-file")["sessionFile"].as_str().unwrap());
    child.finish();
    assert_eq!(child.seen.iter().filter(|frame| frame["type"] == "host_uri_request").count(), 3);
    assert_no_side_channel_ack(&child, &[read["id"].clone(), write["id"].clone(), readonly["id"].clone()]);
    let entries = journal(&file);
    let read_receipt = receipt(&entries, "db-read");
    assert_eq!(read_receipt["isError"], false);
    assert_eq!(read_receipt["details"]["contentType"], "text/markdown");
    assert_eq!(read_receipt["details"]["meta"]["source"], json!({"type":"internal","value":"db://table"}));
    assert!(read_receipt["details"].get("resolvedPath").is_none());
    assert_eq!(text_content(read_receipt), "1:标题😀\n2:next");
    assert_eq!(
        text_content(receipt(&entries, "db-write")),
        "Successfully wrote 8 bytes to db://table\nNote: auto-stripped hashline display prefixes from content before writing."
    );
    assert_eq!(receipt(&entries, "readonly-write")["isError"], true);
    assert!(text_content(receipt(&entries, "readonly-write")).contains("read-only"));
    assert_eq!(receipt(&entries, "readonly-read")["details"]["contentType"], "text/plain");
    assert_eq!(text_content(receipt(&entries, "readonly-read")), "readonly text");
    assert_eq!(
        std::fs::read_dir(env.work.path()).unwrap().count(),
        1,
        "only fixture .git; URIs never materialize files"
    );
    assert_eq!(up.served(), 5);
}

#[tokio::test(flavor = "multi_thread")]
async fn side_channels_after_session_join_and_abort_are_consumed_without_cross_session_completion() {
    let up = upstream(vec![
        calls(&[("same-model-id", "custom", json!({"version":"A"}))]),
        calls(&[("same-model-id", "custom", json!({"version":"A"}))]),
        answer("New session receipt."),
        calls(&[("abort-uri", "read", json!({"path":"db://waiting"}))]),
    ])
    .await;
    let env = Env::new();
    let mut child = RpcChild::spawn(&env, &up, &["--tools", "read"]);
    child.ready();
    child.send(json!({"id":"register","type":"set_host_tools","tools":[definition("custom","A",false)]}));
    child.success("register");
    child.send(json!({"id":"register-db","type":"set_host_uri_schemes","schemes":[{"scheme":"db"}]}));
    child.success("register-db");
    child.prompt("origin-run");
    let old = child.until(|frame| frame["type"] == "host_tool_call");
    let old_file = PathBuf::from(child.state("old-file")["sessionFile"].as_str().unwrap());
    child.send(json!({"id":"join-new","type":"new_session"}));
    child.until(|frame| frame["type"] == "host_tool_cancel" && frame["targetId"] == old["id"]);
    child.send_many(&[
        json!({"type":"host_tool_update","id":old["id"],"partialResult":{"content":[{"type":"text","text":"after queued join"}]}}),
        result(&old["id"], "old session completion", "A"),
    ]);
    assert_eq!(child.success("join-new")["cancelled"], false);
    let state = child.state("new-state");
    let new_file = PathBuf::from(state["sessionFile"].as_str().unwrap());
    assert_ne!(new_file, old_file);
    assert!(state["dumpTools"].as_array().unwrap().iter().any(|tool| tool["name"] == "custom"));
    child.prompt("new-run");
    let fresh = child.until(|frame| frame["type"] == "host_tool_call");
    assert_eq!(fresh["toolCallId"], old["toolCallId"]);
    assert_ne!(fresh["id"], old["id"]);
    child.send(result(&old["id"], "stale origin completion", "stale"));
    assert_eq!(child.state("new-waiting")["isStreaming"], true);
    let begin = child.seen.len();
    child.send(result(&fresh["id"], "new receipt only", "A"));
    child.until(|frame| frame["type"] == "agent_end");
    assert!(
        child.seen[begin..]
            .iter()
            .any(|frame| frame["type"] == "tool_execution_end" && text_content(&frame["result"]) == "new receipt only")
    );
    child.prompt("abort-uri-run");
    let uri = child.until(|frame| frame["type"] == "host_uri_request");
    child.send(json!({"id":"join-abort","type":"abort"}));
    child.until(|frame| frame["type"] == "host_uri_cancel" && frame["targetId"] == uri["id"]);
    child.send(json!({"type":"host_uri_result","id":uri["id"],"content":"after queued abort"}));
    child.success("join-abort");
    assert_eq!(child.state("after-abort")["isStreaming"], false);
    child.finish();
    assert_no_side_channel_ack(&child, &[old["id"].clone(), fresh["id"].clone(), uri["id"].clone()]);
    let old_entries = journal(&old_file);
    let old_receipt = receipt(&old_entries, "same-model-id");
    assert_eq!(old_receipt["isError"], true);
    assert!(text_content(old_receipt).contains("was aborted"), "{old_receipt}");
    assert_native_unknown_settlement(&child, "origin-run");
    let new_entries = journal(&new_file);
    assert_eq!(text_content(receipt(&new_entries, "same-model-id")), "new receipt only");
    assert!(!new_entries.iter().any(|entry| text_content(&entry["message"]).contains("stale origin")));
    receipt(&new_entries, "abort-uri");
    let terminal = child.seen.iter().position(|frame| frame["type"] == "agent_end").unwrap();
    let joined = child.seen.iter().position(|frame| frame["type"] == "response" && frame["id"] == "join-new").unwrap();
    assert!(terminal < joined, "old Run settles before new Session adoption ACK");
    assert_eq!(up.served(), 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn eof_releases_pending_host_and_uri_waits_journals_unknown_and_resume_never_replays() {
    let up = upstream(vec![
        calls(&[("eof-host", "custom", json!({"version":"A"})), ("eof-uri", "read", json!({"path":"db://waiting"}))]),
        answer("EOF drain inspected unknown host receipts."),
        answer("Resume inspected receipts without replay."),
    ])
    .await;
    let env = Env::new();
    let mut child = RpcChild::spawn(&env, &up, &["--tools", "read"]);
    child.ready();
    child.send(json!({"id":"eof-tools","type":"set_host_tools","tools":[definition("custom","A",false)]}));
    child.success("eof-tools");
    child.send(json!({"id":"eof-schemes","type":"set_host_uri_schemes","schemes":[{"scheme":"db"}]}));
    child.success("eof-schemes");
    child.prompt("eof-task");
    let mut pending = Vec::new();
    while pending.len() < 2 {
        let frame = child.next();
        if frame["type"] == "host_tool_call" || frame["type"] == "host_uri_request" {
            pending.push(frame);
        }
    }
    let file = PathBuf::from(child.state("eof-file")["sessionFile"].as_str().unwrap());
    child.finish();
    let entries = journal(&file);
    let host = receipt(&entries, "eof-host");
    assert_eq!(host["isError"], true);
    let host_error = text_content(host);
    assert!(
        ["RPC input disconnected", "RPC host disconnected", "Native Host closed", "was aborted"]
            .iter()
            .any(|reason| host_error.contains(reason)),
        "{host}"
    );
    assert_native_unknown_settlement(&child, "eof-task");
    eprintln!("Durable Native Host error: {host}");
    let uri = receipt(&entries, "eof-uri");
    assert_eq!(uri["isError"], true);
    assert!(text_content(uri).contains("effect unknown"));
    assert_eq!(up.served(), 2);
    let mut reopened = RpcChild::spawn(&env, &up, &["--resume", file.to_str().unwrap(), "--tools", "read"]);
    reopened.ready();
    reopened.send(json!({"id":"restored","type":"get_messages"}));
    let restored = reopened.success("restored");
    assert_eq!(
        restored["messages"].as_array().unwrap().iter().filter(|message| message["role"] == "toolResult").count(),
        2
    );
    for frame in &pending {
        if frame["type"] == "host_tool_call" {
            reopened.send(result(&frame["id"], "late previous connection", "stale"));
        } else {
            reopened.send(json!({"type":"host_uri_result","id":frame["id"],"content":"late previous connection"}));
        }
    }
    reopened.prompt("resume-task");
    reopened.until(|frame| frame["type"] == "agent_end");
    reopened.finish();
    assert!(
        !reopened.seen.iter().any(|frame| frame["type"] == "host_tool_call" || frame["type"] == "host_uri_request")
    );
    assert_no_side_channel_ack(&reopened, &pending.iter().map(|frame| frame["id"].clone()).collect::<Vec<_>>());
    let final_entries = journal(&file);
    receipt(&final_entries, "eof-host");
    receipt(&final_entries, "eof-uri");
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 3);
    for (call_id, durable_text) in [("eof-host", host_error), ("eof-uri", text_content(uri))] {
        let resumed_tools: Vec<_> = requests[2]["body"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "tool" && message["tool_call_id"] == call_id)
            .collect();
        assert_eq!(resumed_tools.len(), 1, "restore retains one original receipt for {call_id}");
        assert_eq!(text_content(resumed_tools[0]), durable_text);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_side_channel_guards_return_ordinary_errors_but_valid_unknown_replies_have_no_ack() {
    let up = upstream(Vec::new()).await;
    let env = Env::new();
    let mut child = RpcChild::spawn(&env, &up, &["--tools", "read"]);
    child.ready();
    child.send_many(&[
        json!({"type":"host_tool_result","id":"bad-result","result":{"content":{}}}),
        json!({"type":"host_tool_update","id":"bad-update","partialResult":{"content":"not an array"}}),
        json!({"type":"host_uri_result","id":42,"content":"numeric id"}),
    ]);
    child.error("bad-result");
    child.error("bad-update");
    let numeric = child.until(|frame| frame["type"] == "response" && frame["id"] == 42);
    assert_eq!(numeric["success"], false);
    child.send_many(&[
        json!({"type":"host_tool_result","id":"unknown-tool","result":{"content":[]}}),
        json!({"type":"host_tool_update","id":"unknown-update","partialResult":{"content":[]}}),
        json!({"type":"host_uri_result","id":"unknown-uri"}),
    ]);
    assert_eq!(child.state("guard-barrier")["isStreaming"], false);
    child.finish();
    assert_no_side_channel_ack(&child, &[json!("unknown-tool"), json!("unknown-update"), json!("unknown-uri")]);
    assert_eq!(up.served(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn eof_before_accepted_registration_and_prompt_rejects_future_uri_without_dispatch() {
    let mut delayed = calls(&[("after-eof-uri", "read", json!({"path":"db://waiting"}))]);
    // Hold the model response while the stdin reader observes EOF. Scheme
    // registration may be processed either before or after that observation;
    // both paths must reject the later read without publishing a request.
    delayed["delay_ms"] = json!(100);
    let up = upstream(vec![delayed, answer("Queued task inspected the predispatch URI rejection.")]).await;
    let env = Env::new();
    let mut child = RpcChild::spawn(&env, &up, &["--tools", "read"]);
    child.ready();
    child.send_many(&[
        json!({"id":"queued-db","type":"set_host_uri_schemes","schemes":[{"scheme":"db"}]}),
        json!({"id":"queued-run","type":"prompt","message":"accepted work after input EOF"}),
    ]);
    child.finish();
    assert!(
        child
            .seen
            .iter()
            .any(|frame| frame["type"] == "response" && frame["id"] == "queued-db" && frame["success"] == true)
    );
    assert!(
        child
            .seen
            .iter()
            .any(|frame| frame["type"] == "response" && frame["id"] == "queued-run" && frame["success"] == true)
    );
    assert!(
        !child.seen.iter().any(|frame| frame["type"] == "host_uri_request" || frame["type"] == "host_uri_cancel"),
        "an EOF connection cannot publish a new request or create an orphan waiter"
    );
    let files = std::fs::read_dir(&env.sessions)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "jsonl"))
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 1);
    let entries = journal(&files[0]);
    let rejected = receipt(&entries, "after-eof-uri");
    assert_eq!(rejected["isError"], true);
    let error = text_content(rejected);
    // EOF removes registered routes; re-registration after EOF can restore the
    // route, but dispatch still rejects the permanently closed connection.
    assert!(
        error == "Unknown protocol: db://" || (error.contains("before dispatch") && error.contains("not executed")),
        "{error}"
    );
    assert!(!error.contains("effect unknown"), "absence of publication is proven");
    assert!(entries.iter().any(|entry| text_content(&entry["message"]).contains("Queued task inspected")));
    assert_eq!(up.served(), 2);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert!(requests[1]["body"]["messages"].as_array().unwrap().iter().any(|message| message["role"] == "tool"
        && message["tool_call_id"] == "after-eof-uri"
        && text_content(message) == error));
}

// Production ACP observations stay in this original coarse Host family. The
// child is the actual ara binary; only its HTTP model and peer UI/files are
// controlled. None of the authoring helpers below is a separate test gate.
fn acp_env(plan_enabled: bool) -> Env {
    let mut env = Env::new();
    // Multiple real Native Session preparations share this one case deadline.
    env.deadline = Instant::now() + Duration::from_secs(20);
    let directory = env.home.path().join("agent");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("config.yml"),
        format!("lsp:\n  enabled: false\nedit:\n  enforceSeenLines: false\nplan:\n  enabled: {plan_enabled}\n"),
    )
    .unwrap();
    env
}

fn acp_session_file(env: &Env, id: &str) -> PathBuf {
    std::fs::read_dir(&env.sessions)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "jsonl"))
        .find(|path| journal(path).iter().any(|entry| entry["type"] == "session" && entry["id"] == id))
        .unwrap_or_else(|| panic!("Missing actual ACP Native journal for {id}"))
}

fn acp_saved_fixture(env: &Env) -> (String, PathBuf, PathBuf) {
    std::fs::create_dir_all(&env.sessions).unwrap();
    let mut saved = ara_session::SessionJournal::create(&env.sessions, env.work.path()).unwrap();
    saved.materialize().unwrap();
    let id = saved.session_id().to_owned();
    let file = saved.path().to_owned();
    let artifacts = ara_cli::session_artifacts::SessionArtifacts::for_journal(&saved);
    let local = ara_cli::mcp_native::local::session_root(artifacts.directory(), &id).unwrap();
    (id, file, local)
}

fn has_model_tool(request: &Value, name: &str) -> bool {
    request["body"]["tools"].as_array().into_iter().flatten().any(|tool| tool["function"]["name"] == name)
}

fn terminal_acp_tool(frame: &Value, call: &str) -> bool {
    frame["method"] == "session/update"
        && frame["params"]["update"]["sessionUpdate"] == "tool_call_update"
        && frame["params"]["update"]["toolCallId"] == call
        && matches!(frame["params"]["update"]["status"].as_str(), Some("completed" | "failed"))
}

#[tokio::test(flavor = "multi_thread")]
async fn acp_replace_patch_real_peer_and_capability_off_keep_owned_native_receipts() {
    // B-355ae274b5 / B-cfb3047963 and their real initialize-capability control.
    for (mode, before, args, after, bridge) in [
        (
            "replace",
            "old content\n",
            json!({"path":"target.txt","old_string":"old content","new_string":"new content"}),
            "new content\n",
            true,
        ),
        ("patch", "a\n", json!({"path":"target.txt","edits":[{"op":"update","diff":"@@\n-a\n+b"}]}), "b\n", true),
        (
            "replace",
            "old content\n",
            json!({"path":"target.txt","old_string":"old content","new_string":"new content"}),
            "new content\n",
            false,
        ),
    ] {
        let env = acp_env(true);
        let target = env.work.path().join("target.txt");
        std::fs::write(&target, before).unwrap();
        let up = upstream(vec![calls(&[("acp-edit", "edit", args)]), answer("Actual peer receipt inspected.")]).await;
        let mut child = RpcChild::spawn_acp(&env, &up, &["--tools", "read,edit", "--edit-mode", mode]);
        child.acp_initialize(json!({"fs":{"writeTextFile":bridge}}));
        let opened = child.acp_new(env.work.path());
        let id = opened["sessionId"].as_str().unwrap();
        assert_eq!(opened["modes"]["currentModeId"], "default");
        child.acp_prompt("edit-run", id);
        if bridge {
            child.peer_write(id, &target, after, false);
        }
        assert_eq!(child.acp_success("edit-run")["stopReason"], "end_turn");
        let output = child.acp_result("acp-edit");
        assert_eq!(output["details"]["newText"], after);
        assert_ne!(output["isError"], true, "{output}");
        child.join_after_eof(0);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), after);
        assert_eq!(
            child.seen.iter().filter(|frame| frame["method"] == "fs/write_text_file").count(),
            usize::from(bridge)
        );
        let entries = journal(&acp_session_file(&env, id));
        assert_eq!(receipt(&entries, "acp-edit")["isError"], false);
        assert_eq!(receipt(&entries, "acp-edit")["details"]["newText"], after);
        let requests = up.requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert!(
            requests[1]["body"]["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| message["role"] == "tool" && message["tool_call_id"] == "acp-edit")
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn acp_hashline_real_peer_reports_persisted_drift_and_exact_tags() {
    // B-0d8fd8d258 / B-bb4f3600f3: the actual peer writes the formatted bytes.
    let env = acp_env(true);
    let drift = "function f() {\n    return 1;\n}\n";
    let exact = "hello\nworld\n";
    std::fs::write(env.work.path().join("drift.ts"), drift).unwrap();
    std::fs::write(env.work.path().join("exact.txt"), exact).unwrap();
    let up = upstream(vec![
        calls(&[(
            "acp-drift",
            "edit",
            json!({"input":format!("[drift.ts#{}]\nPUT 2-2:\n+    return 2;",ara_edit::store::file_hash(drift))}),
        )]),
        answer("Persisted formatting inspected."),
        calls(&[(
            "acp-exact",
            "edit",
            json!({"input":format!("[exact.txt#{}]\nPUT 2-2:\n+earth",ara_edit::store::file_hash(exact))}),
        )]),
        answer("Exact saved bytes inspected."),
    ])
    .await;
    let mut child = RpcChild::spawn_acp(&env, &up, &["--tools", "read,edit", "--edit-mode", "hashline"]);
    child.acp_initialize(json!({"fs":{"writeTextFile":true}}));
    let opened = child.acp_new(env.work.path());
    let id = opened["sessionId"].as_str().unwrap();
    for (prompt, call, name, requested, persisted, formatted) in [
        (
            "drift-run",
            "acp-drift",
            "drift.ts",
            "function f() {\n    return 2;\n}\n",
            "function f() {\n\treturn 2;\n}\n",
            true,
        ),
        ("exact-run", "acp-exact", "exact.txt", "hello\nearth\n", "hello\nearth\n", false),
    ] {
        child.acp_prompt(prompt, id);
        child.peer_write(id, &env.work.path().join(name), requested, formatted);
        assert_eq!(child.acp_success(prompt)["stopReason"], "end_turn");
        let output = child.acp_result(call);
        assert_ne!(output["isError"], true, "{output}");
        assert_eq!(output["details"]["newText"], persisted);
        let rendered = text_content(&output);
        assert_eq!(rendered.contains("reformatted it on save"), formatted);
        assert!(rendered.contains(&format!("#{}]", ara_edit::store::file_hash(persisted))), "{rendered}");
        assert_eq!(std::fs::read_to_string(env.work.path().join(name)).unwrap(), persisted);
    }
    child.join_after_eof(0);
    let entries = journal(&acp_session_file(&env, id));
    assert_eq!(receipt(&entries, "acp-drift")["details"]["newText"], "function f() {\n\treturn 2;\n}\n");
    assert_eq!(receipt(&entries, "acp-exact")["details"]["newText"], "hello\nearth\n");
    assert_eq!(child.seen.iter().filter(|frame| frame["method"] == "fs/write_text_file").count(), 2);
    assert_eq!(up.served(), 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn acp_plan_local_absolute_bypass_real_modes_hidden_write_restore_and_identity_isolation() {
    // B-3856d00e02: a real saved Native Session gives the absolute local path
    // before constructing the controlled model script; ACP itself loads it.
    let env = acp_env(true);
    // This supplemental visibility matrix isolates plan-only WRITE admission.
    // Fixed suppliers otherwise add device-only WRITE for default xdev and
    // auto-enabled astEdit; those defaults are covered by the other ACP groups.
    let config = env.home.path().join("agent/config.yml");
    let settings = std::fs::read_to_string(&config).unwrap();
    std::fs::write(&config, format!("{settings}tools:\n  xdev: false\nastEdit:\n  enabled: false\n")).unwrap();
    let (id, file, local) = acp_saved_fixture(&env);
    let absolute_plan = local.join("PLAN.md");
    let notes = local.join("notes.md");
    let outside = env.work.path().join("outside.txt");
    std::fs::write(&outside, "untouched\n").unwrap();
    let up = upstream(vec![
        answer("Selected tools inspected before plan."),
        calls(&[
            ("plan-local-write", "write", json!({"path":"local://PLAN.md","content":"# Existing\nold plan\n"})),
            (
                "plan-local-edit",
                "edit",
                json!({"path":"local://PLAN.md","old_string":"old plan","new_string":"new plan"}),
            ),
            (
                "plan-absolute-edit",
                "edit",
                json!({"path":absolute_plan,"old_string":"new plan","new_string":"absolute plan"}),
            ),
            ("plan-absolute-write", "write", json!({"path":notes,"content":"# Notes\nsession-local\n"})),
            (
                "plan-denied-edit",
                "edit",
                json!({"path":"outside.txt","old_string":"untouched","new_string":"must not change"}),
            ),
        ]),
        answer("Session-local plan bytes inspected."),
        answer("Original selected tools restored."),
    ])
    .await;
    let mut child = RpcChild::spawn_acp(&env, &up, &["--tools", "read,edit", "--edit-mode", "replace"]);
    child.acp_initialize(json!({"fs":{"writeTextFile":true}}));
    child.acp_request("load-plan", "session/load", json!({"sessionId":id,"cwd":env.work.path(),"mcpServers":[]}));
    child.acp_prompt("before-plan", &id);
    child.acp_success("before-plan");
    child.acp_request("enter-plan", "session/set_mode", json!({"sessionId":id,"modeId":"plan"}));
    child.acp_prompt("local-plan", &id);
    assert_eq!(child.acp_success("local-plan")["stopReason"], "end_turn");
    for call in ["plan-local-write", "plan-local-edit", "plan-absolute-edit", "plan-absolute-write"] {
        assert_ne!(child.acp_result(call)["isError"], true, "{call}");
    }
    assert!(text_content(&child.acp_result("plan-denied-edit")).contains("working tree is read-only"));
    assert_eq!(std::fs::read_to_string(&absolute_plan).unwrap(), "# Existing\nabsolute plan\n");
    assert_eq!(std::fs::read_to_string(&notes).unwrap(), "# Notes\nsession-local\n");
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "untouched\n");
    assert!(!child.seen.iter().any(|frame| frame["method"] == "fs/write_text_file"));
    let resumed = child.acp_request(
        "retained-plan",
        "session/resume",
        json!({"sessionId":id,"cwd":env.work.path(),"mcpServers":[]}),
    );
    assert_eq!(resumed["modes"]["currentModeId"], "plan");
    let other_cwd = tempfile::tempdir().unwrap();
    child.acp_send("wrong-cwd", "session/load", json!({"sessionId":id,"cwd":other_cwd.path(),"mcpServers":[]}));
    assert!(child.acp_response("wrong-cwd")["error"]["message"].as_str().unwrap().contains("cwd does not match"));
    let fresh = child.acp_new(env.work.path());
    assert_ne!(fresh["sessionId"], id);
    assert_eq!(fresh["modes"]["currentModeId"], "default");
    let origin = child.acp_request(
        "origin-still-plan",
        "session/load",
        json!({"sessionId":id,"cwd":env.work.path(),"mcpServers":[]}),
    );
    assert_eq!(origin["modes"]["currentModeId"], "plan");
    let exited = child.acp_request(
        "exit-plan-config",
        "session/set_config_option",
        json!({"sessionId":id,"configId":"mode","value":"default"}),
    );
    assert_eq!(exited["configOptions"][0]["currentValue"], "default");
    child.acp_prompt("after-plan", &id);
    child.acp_success("after-plan");
    child.join_after_eof(0);
    let entries = journal(&file);
    assert_eq!(receipt(&entries, "plan-local-edit")["details"]["newText"], "# Existing\nnew plan\n");
    assert_eq!(receipt(&entries, "plan-absolute-edit")["details"]["newText"], "# Existing\nabsolute plan\n");
    assert_eq!(receipt(&entries, "plan-denied-edit")["isError"], true);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 4);
    assert!(!has_model_tool(&requests[0], "write"));
    assert!(has_model_tool(&requests[1], "write"));
    assert!(requests[1]["body"]["messages"].to_string().contains("Plan mode active."));
    assert!(!has_model_tool(&requests[3], "write"));
    assert!(
        requests[3]["body"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| text_content(message).contains("# Existing\nabsolute plan"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn acp_plan_form_refine_cancel_unknown_approve_and_durable_reference_use_real_peer() {
    let env = acp_env(true);
    let body = "# Feature\n1. Implement the selected change.\n2. Verify its result.\n";
    let up = upstream(vec![
        calls(&[("draft-feature", "write", json!({"path":"local://feature-plan.md","content":body}))]),
        calls(&[("proposal-refine", "write", json!({"path":"xd://propose","content":"feature"}))]),
        calls(&[("proposal-cancel", "write", json!({"path":"xd://propose","content":"feature"}))]),
        calls(&[("proposal-unknown", "write", json!({"path":"xd://propose","content":"feature"}))]),
        calls(&[("proposal-approve", "write", json!({"path":"xd://propose","content":"feature"}))]),
        answer("Approval receipt and full tool restoration inspected."),
        answer("Durable inline approved reference inspected."),
        answer("Already admitted reference retained."),
    ])
    .await;
    let mut child = RpcChild::spawn_acp(&env, &up, &["--tools", "read"]);
    child.acp_initialize(json!({"fs":{"writeTextFile":true},"elicitation":{"form":{}}}));
    let opened = child.acp_new(env.work.path());
    let id = opened["sessionId"].as_str().unwrap();
    child.acp_request(
        "form-enter",
        "session/set_config_option",
        json!({"sessionId":id,"configId":"mode","value":"plan"}),
    );
    child.acp_prompt("form-run", id);
    let mut cursor = 0;
    let mut prior = None;
    for (call, choice, approve) in [
        ("proposal-refine", json!({"action":"accept","content":{"value":"Refine plan"}}), false),
        ("proposal-cancel", json!({"action":"cancel"}), false),
        ("proposal-unknown", json!({"action":"accept","content":{"value":"unknown choice"}}), false),
        ("proposal-approve", json!({"action":"accept","content":{"value":"Approve and execute"}}), true),
    ] {
        let request = child.peer_request("elicitation/create", &mut cursor);
        assert_eq!(request["jsonrpc"], "2.0");
        assert_eq!(
            request["params"],
            json!({"mode":"form","sessionId":id,
            "message":format!("Approve plan \"feature\" and start implementation?\n\n{body}"),
            "requestedSchema":{"type":"object","properties":{"value":{"type":"string",
                "enum":["Approve and execute","Refine plan"]}},"required":["value"]}})
        );
        if let Some(stale) = prior.take() {
            child.send(json!({"jsonrpc":"2.0","id":stale,"result":{"action":"accept","content":{"value":"Approve and execute"}}}));
            // History replay waits for the whole active prompt. Inspect the
            // retained mode without asking it to replay the pending Run.
            let barrier = child.acp_request(
                &format!("stale-barrier-{call}"),
                "session/resume",
                json!({"sessionId":id,"cwd":env.work.path(),"mcpServers":[]}),
            );
            assert_eq!(barrier["modes"]["currentModeId"], "plan", "late prior reply cannot approve a new request");
        }
        child.send(json!({"jsonrpc":"2.0","id":request["id"],"result":choice}));
        child.until(|frame| terminal_acp_tool(frame, call));
        let output = child.acp_result(call);
        assert_ne!(output["isError"], true, "{output}");
        assert_eq!(
            output["details"]["xdev"],
            json!({"tool":"propose","mode":"execute","args":{"title":"feature"},
            "inner":{"planFilePath":"local://feature-plan.md","title":"feature","planExists":true}})
        );
        assert_eq!(text_content(&output).contains("Plan approved at"), approve);
        assert_eq!(text_content(&output).contains("Plan refinement requested."), !approve);
        if !approve {
            let state = child.acp_request(
                &format!("mode-after-{call}"),
                "session/resume",
                json!({"sessionId":id,"cwd":env.work.path(),"mcpServers":[]}),
            );
            assert_eq!(state["modes"]["currentModeId"], "plan");
            prior = Some(request["id"].clone());
        }
    }
    assert_eq!(child.acp_success("form-run")["stopReason"], "end_turn");
    let state = child.acp_request(
        "approved-mode",
        "session/load",
        json!({"sessionId":id,"cwd":env.work.path(),"mcpServers":[]}),
    );
    assert_eq!(state["modes"]["currentModeId"], "default");
    for prompt in ["reference-next", "reference-retained"] {
        child.acp_prompt(prompt, id);
        child.acp_success(prompt);
    }
    child.join_after_eof(0);
    assert_eq!(child.seen.iter().filter(|frame| frame["method"] == "elicitation/create").count(), 4);
    assert!(!child.seen.iter().any(|frame| frame["method"] == "fs/write_text_file"));
    assert_eq!(
        child
            .seen
            .iter()
            .filter(|frame| frame["params"]["update"]["sessionUpdate"] == "current_mode_update"
                && frame["params"]["update"]["currentModeId"] == "default")
            .count(),
        1
    );
    let file = acp_session_file(&env, id);
    let entries = journal(&file);
    for call in ["proposal-refine", "proposal-cancel", "proposal-unknown", "proposal-approve"] {
        assert_eq!(receipt(&entries, call)["details"]["xdev"]["inner"]["planFilePath"], "local://feature-plan.md");
    }
    let reference = entries
        .iter()
        .filter(|entry| {
            entry["customType"] == "plan-mode-reference" || entry["message"]["customType"] == "plan-mode-reference"
        })
        .collect::<Vec<_>>();
    assert_eq!(
        reference.len(),
        1,
        "approved reference is journaled once after actual next-input admission: {entries:?}"
    );
    assert!(reference[0].to_string().contains("Feature"));
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 8);
    assert!(has_model_tool(&requests[0], "write"));
    assert!(!has_model_tool(&requests[5], "write"));
    assert!(!requests[5]["body"]["messages"].to_string().contains("Approved plan inlined below"));
    assert!(requests[6]["body"]["messages"].to_string().contains("Approved plan inlined below"));
}

#[tokio::test(flavor = "multi_thread")]
async fn acp_plan_without_form_autoapproves_and_disabled_snapshot_rejects_both_mode_apis() {
    let env = acp_env(true);
    let up = upstream(vec![
        calls(&[(
            "auto-draft",
            "write",
            json!({"path":"local://auto-plan.md","content":"# Auto\nImplement the accepted work.\n"}),
        )]),
        calls(&[("auto-propose", "write", json!({"path":"xd://propose","content":"auto"}))]),
        answer("No-form approval completed through the actual proposal device."),
    ])
    .await;
    let mut child = RpcChild::spawn_acp(&env, &up, &["--tools", "read"]);
    child.acp_initialize(json!({"fs":{"writeTextFile":true}}));
    let opened = child.acp_new(env.work.path());
    let id = opened["sessionId"].as_str().unwrap();
    child.acp_request("auto-enter", "session/set_mode", json!({"sessionId":id,"modeId":"plan"}));
    child.acp_prompt("auto-run", id);
    child.acp_success("auto-run");
    let output = child.acp_result("auto-propose");
    assert!(text_content(&output).contains("Plan approved at local://auto-plan.md"));
    assert_eq!(
        output["details"]["xdev"]["inner"],
        json!({"planFilePath":"local://auto-plan.md","title":"auto","planExists":true})
    );
    let state = child.acp_request(
        "auto-exited",
        "session/resume",
        json!({"sessionId":id,"cwd":env.work.path(),"mcpServers":[]}),
    );
    assert_eq!(state["modes"]["currentModeId"], "default");
    child.join_after_eof(0);
    assert!(
        !child
            .seen
            .iter()
            .any(|frame| frame["method"] == "elicitation/create" || frame["method"] == "fs/write_text_file")
    );
    assert_eq!(receipt(&journal(&acp_session_file(&env, id)), "auto-propose")["isError"], false);

    let disabled = acp_env(false);
    let unused = upstream(Vec::new()).await;
    let mut child = RpcChild::spawn_acp(&disabled, &unused, &["--tools", "read"]);
    child.acp_initialize(json!({"fs":{"writeTextFile":true},"elicitation":{"form":{}}}));
    let opened = child.acp_new(disabled.work.path());
    let id = opened["sessionId"].as_str().unwrap();
    assert_eq!(opened["modes"]["availableModes"].as_array().unwrap().len(), 1);
    for (request, method, fields) in [
        ("disabled-mode", "session/set_mode", json!({"sessionId":id,"modeId":"plan"})),
        ("disabled-config", "session/set_config_option", json!({"sessionId":id,"configId":"mode","value":"plan"})),
    ] {
        child.acp_send(request, method, fields);
        assert!(child.acp_response(request)["error"]["message"].as_str().unwrap().contains("Unsupported ACP mode"));
    }
    child.join_after_eof(0);
    assert_eq!(unused.served(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn acp_eof_settles_owned_peer_effect_and_held_load_without_replay_or_fallback() {
    let env = acp_env(true);
    let target = env.work.path().join("held.txt");
    std::fs::write(&target, "before\n").unwrap();
    let up = upstream(vec![
        calls(&[("held-peer-edit", "edit", json!({"path":"held.txt","old_string":"before","new_string":"after"}))]),
        answer("Unknown previous peer effect inspected without another write."),
        answer("Resumed receipt inspected without another write."),
    ])
    .await;
    let mut child = RpcChild::spawn_acp(&env, &up, &["--tools", "read,edit", "--edit-mode", "replace"]);
    child.acp_initialize(json!({"fs":{"writeTextFile":true}}));
    let opened = child.acp_new(env.work.path());
    let id = opened["sessionId"].as_str().unwrap();
    child.acp_prompt("held-write-run", id);
    let pending = child.until(|frame| frame["method"] == "fs/write_text_file");
    assert_eq!(pending["params"]["sessionId"], id);
    assert_eq!(pending["params"]["content"], "after\n");
    // Peer received the request but holds its result and performs no write.
    // EOF cannot prove an effect; the owning Host must journal Unknown.
    child.join_after_eof(0);
    let file = acp_session_file(&env, id);
    let entries = journal(&file);
    let interrupted = receipt(&entries, "held-peer-edit");
    assert_eq!(interrupted["isError"], true);
    assert!(text_content(interrupted).to_ascii_lowercase().contains("unknown"), "{interrupted}");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "before\n");
    assert_eq!(child.seen.iter().filter(|frame| frame["method"] == "fs/write_text_file").count(), 1);
    let mut reopened = RpcChild::spawn_acp(&env, &up, &["--tools", "read,edit", "--edit-mode", "replace"]);
    reopened.acp_initialize(json!({"fs":{"writeTextFile":true}}));
    reopened.acp_request(
        "resume-held",
        "session/resume",
        json!({"sessionId":id,"cwd":env.work.path(),"mcpServers":[]}),
    );
    reopened.send(json!({"jsonrpc":"2.0","id":pending["id"],"result":{}}));
    reopened.acp_request(
        "late-reply-barrier",
        "session/load",
        json!({"sessionId":id,"cwd":env.work.path(),"mcpServers":[]}),
    );
    reopened.acp_prompt("resume-inspect", id);
    reopened.acp_success("resume-inspect");
    reopened.join_after_eof(0);
    assert!(!reopened.seen.iter().any(|frame| frame["method"] == "fs/write_text_file"));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "before\n");
    let final_entries = journal(&file);
    assert_eq!(receipt(&final_entries, "held-peer-edit"), interrupted);
    let requests = up.requests.lock().await;
    let last = requests.last().unwrap();
    assert!(last["body"]["messages"].as_array().unwrap().iter().any(|message| message["role"] == "tool"
        && message["tool_call_id"] == "held-peer-edit"
        && text_content(message).to_ascii_lowercase().contains("unknown")));
    drop(requests);

    // Reuse the real saved identity and the same case deadline. The partial
    // text update proves the model HTTP request is actively held; the server
    // keeps its SSE socket open until cancellation closes it.
    let held = upstream(vec![
        json!({"events":[text("Model HTTP held for concurrent load and EOF.")],"end":"hang"}),
        json!({"events":[text("Model HTTP held for explicit ACP cancellation.")],"end":"hang"}),
        answer("Next prompt after explicit cancellation completed."),
        json!({"status":400,"body":json!({"error":{"message":"controlled ACP failed prompt","type":"invalid_request_error"}}).to_string()}),
        answer("Next prompt after real model failure completed."),
    ]).await;
    let mut active = RpcChild::spawn_acp(&env, &held, &["--tools", "read,edit", "--edit-mode", "replace"]);
    active.acp_initialize(json!({"fs":{"writeTextFile":true}}));
    active.acp_request(
        "held-load-adopt",
        "session/resume",
        json!({"sessionId":id,"cwd":env.work.path(),"mcpServers":[]}),
    );
    active.acp_prompt("model-held-run", id);
    active.until(|frame| {
        frame["method"] == "session/update"
            && frame["params"]["sessionId"] == id
            && frame["params"]["update"]["sessionUpdate"] == "agent_message_chunk"
            && frame["params"]["update"]["content"]["text"] == "Model HTTP held for concurrent load and EOF."
    });
    active.acp_send("held-current-load", "session/load", json!({"sessionId":id,"cwd":env.work.path(),"mcpServers":[]}));
    // A separately dispatched request confirms input remains live while the
    // load's history replay waits behind the current Native prompt lifetime.
    active.acp_request("held-input-barrier", "authenticate", json!({"methodId":"agent"}));
    assert!(!active.seen.iter().any(|frame| frame["id"] == "held-current-load" && frame.get("method").is_none()));
    assert_eq!(held.served(), 1);
    active.join_after_eof(0);
    assert!(!active.seen.iter().any(|frame| frame["method"] == "fs/write_text_file"));
    assert_eq!(held.served(), 1, "EOF joins the held owner without replaying its model request");
    let settled = journal(&file);
    assert_eq!(receipt(&settled, "held-peer-edit"), interrupted);
    assert_eq!(
        settled
            .iter()
            .filter(|entry| entry["message"]["role"] == "user"
                && text_content(&entry["message"]) == "Real ACP bridge task model-held-run")
            .count(),
        1
    );
    assert!(
        settled.iter().any(|entry| entry["message"]["role"] == "assistant"
            && entry["message"]["stopReason"] == "aborted"
            && text_content(&entry["message"]).contains("Model HTTP held for concurrent load and EOF.")),
        "{settled:?}"
    );

    let mut restored = RpcChild::spawn_acp(&env, &held, &["--tools", "read,edit", "--edit-mode", "replace"]);
    restored.acp_initialize(json!({"fs":{"writeTextFile":true}}));
    let state = restored.acp_request(
        "load-after-model-eof",
        "session/load",
        json!({"sessionId":id,"cwd":env.work.path(),"mcpServers":[]}),
    );
    assert_eq!(state["modes"]["currentModeId"], "default");
    assert_eq!(held.served(), 1, "restoring the interrupted history performs no model request");

    restored.acp_prompt("explicit-cancel-run", id);
    restored.until(|frame| {
        frame["method"] == "session/update"
            && frame["params"]["sessionId"] == id
            && frame["params"]["update"]["sessionUpdate"] == "agent_message_chunk"
            && frame["params"]["update"]["content"]["text"] == "Model HTTP held for explicit ACP cancellation."
    });
    restored.acp_send("explicit-cancel", "session/cancel", json!({"sessionId":id}));
    assert_eq!(restored.acp_success("explicit-cancel"), json!({}));
    assert_eq!(restored.acp_success("explicit-cancel-run"), json!({"stopReason":"cancelled"}));
    let state = restored.acp_request(
        "load-after-explicit-cancel",
        "session/load",
        json!({"sessionId":id,"cwd":env.work.path(),"mcpServers":[]}),
    );
    assert_eq!(state["modes"]["currentModeId"], "default");
    assert_eq!(held.served(), 2, "cancel and subsequent load never replay the held HTTP request");
    restored.acp_prompt("after-explicit-cancel", id);
    assert_eq!(restored.acp_success("after-explicit-cancel"), json!({"stopReason":"end_turn"}));

    // A non-retryable real HTTP failure stays a JSON-RPC error, releases the
    // prompt waiter, and permits history loading and a genuinely new prompt.
    restored.acp_prompt("real-failed-prompt", id);
    let failure = restored.acp_response("real-failed-prompt");
    assert_eq!(failure["error"]["code"], -32603, "{failure}");
    assert!(failure["error"]["message"].as_str().unwrap().contains("controlled ACP failed prompt"), "{failure}");
    assert!(failure.get("result").is_none(), "a failed Run cannot become cancelled/success: {failure}");
    let state = restored.acp_request(
        "load-after-real-failure",
        "session/load",
        json!({"sessionId":id,"cwd":env.work.path(),"mcpServers":[]}),
    );
    assert_eq!(state["modes"]["currentModeId"], "default");
    restored.acp_prompt("after-real-failure", id);
    assert_eq!(restored.acp_success("after-real-failure"), json!({"stopReason":"end_turn"}));
    restored.join_after_eof(0);
    assert_eq!(held.served(), 5);
    assert!(!restored.seen.iter().any(|frame| frame["method"] == "fs/write_text_file"));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "before\n");
    let settled = journal(&file);
    assert_eq!(receipt(&settled, "held-peer-edit"), interrupted);
    for prompt in
        ["model-held-run", "explicit-cancel-run", "after-explicit-cancel", "real-failed-prompt", "after-real-failure"]
    {
        assert_eq!(
            settled
                .iter()
                .filter(|entry| entry["message"]["role"] == "user"
                    && text_content(&entry["message"]) == format!("Real ACP bridge task {prompt}"))
                .count(),
            1
        );
    }
    assert!(
        settled.iter().any(|entry| entry["message"]["role"] == "assistant"
            && entry["message"]["stopReason"] == "aborted"
            && text_content(&entry["message"]).contains("Model HTTP held for explicit ACP cancellation.")),
        "{settled:?}"
    );
    assert!(
        settled.iter().any(|entry| entry["message"]["role"] == "assistant"
            && entry["message"]["stopReason"] == "error"
            && entry["message"]["errorMessage"].as_str().unwrap_or("").contains("controlled ACP failed prompt")),
        "{settled:?}"
    );
    assert!(
        settled
            .iter()
            .any(|entry| text_content(&entry["message"]) == "Next prompt after real model failure completed.")
    );
    let requests = held.requests.lock().await;
    assert_eq!(requests.len(), 5);
    for (request, prompt) in requests.iter().zip([
        "model-held-run",
        "explicit-cancel-run",
        "after-explicit-cancel",
        "real-failed-prompt",
        "after-real-failure",
    ]) {
        let user = request["body"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .find(|message| message["role"] == "user")
            .unwrap();
        assert_eq!(text_content(user), format!("Real ACP bridge task {prompt}"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn acp_read_full_editor_buffer_real_peer_capability_failure_and_snapshot_controls() {
    // Fixed original B-d05f349070/B-9f4b0eba55 plus actual ACP consumer controls.
    for (enabled, failed) in [(true, false), (false, false), (true, true)] {
        let env = acp_env(false);
        let target = env.work.path().join("range.txt");
        std::fs::write(&target, "disk one\ndisk two\ndisk three\n").unwrap();
        let up = upstream(vec![
            calls(&[("acp-read", "read", json!({"path":"range.txt:raw:2-2"}))]),
            answer("Actual editor read receipt inspected."),
        ])
        .await;
        let mut child = RpcChild::spawn_acp(&env, &up, &["--tools", "read,edit", "--edit-mode", "hashline"]);
        child.acp_initialize(json!({"fs":{"readTextFile":enabled,"writeTextFile":false}}));
        let opened = child.acp_new(env.work.path());
        let id = opened["sessionId"].as_str().unwrap();
        child.acp_prompt("read-run", id);
        if enabled {
            let mut cursor = 0;
            let request = child.peer_request("fs/read_text_file", &mut cursor);
            assert_eq!(request["params"]["sessionId"], id);
            assert_eq!(request["params"]["path"], target.to_string_lossy().to_string());
            assert!(request["params"].get("line").is_none() && request["params"].get("limit").is_none());
            if failed {
                child.send(
                    json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32603,"message":"editor read failed"}}),
                );
            } else {
                child.send(json!({"jsonrpc":"2.0","id":request["id"],"result":{"content":"bridge one\nbridge two\nbridge three\n"}}));
            }
        }
        assert_eq!(child.acp_success("read-run")["stopReason"], "end_turn");
        let output = child.acp_result("acp-read");
        let expected = if enabled && !failed { "bridge two" } else { "disk two" };
        let actual = text_content(&output);
        if enabled && !failed {
            assert!(actual.starts_with(&format!("{expected}\n\n[")), "{output}");
            assert!(actual.contains("more lines in file. Use :3 to continue]"));
        } else {
            assert_eq!(actual, expected, "fixed ordinary disk raw fallback has no inline continuation");
        }
        assert!(!actual.contains(if enabled && !failed { "disk two" } else { "bridge two" }));
        assert_ne!(output["isError"], true);
        child.join_after_eof(0);
        assert_eq!(
            child.seen.iter().filter(|frame| frame["method"] == "fs/read_text_file").count(),
            usize::from(enabled)
        );
        assert!(!child.seen.iter().any(|frame| frame["method"] == "fs/write_text_file"));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "disk one\ndisk two\ndisk three\n");
        let entries = journal(&acp_session_file(&env, id));
        assert_eq!(receipt(&entries, "acp-read")["isError"], false);
        assert_eq!(text_content(receipt(&entries, "acp-read")), actual);
        let requests = up.requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert!(requests[1]["body"]["messages"].as_array().unwrap().iter().any(|message| message["role"] == "tool"
            && message["tool_call_id"] == "acp-read"
            && message["content"] == actual));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn sqlite_read_actual_rpc_acp_unicode_text_owner_and_receipts() {
    // Supplemental production metrics-owner integration, not a fake renderer.
    // CLI/RPC/ACP all bind through main::new_tool_context; actual RPC and ACP
    // children exercise the shared constructor rather than injecting test metrics.
    for acp in [false, true] {
        let env = Env::new();
        let target = env.work.path().join("unicode.sqlite");
        let db = rusqlite::Connection::open(&target).unwrap();
        db.execute_batch("CREATE TABLE entries (\"姓名\" TEXT, emoji TEXT, ansi TEXT, payload BLOB, large INTEGER)").unwrap();
        let first = "中文 e\u{301} 👨‍👩‍👧‍👦";
        db.execute("INSERT INTO entries VALUES (?,?,?,?,?)", rusqlite::params![first, "👍🏽".repeat(35), "\u{1b}[31mred\u{1b}[0m", vec![0u8;1280], i64::MAX]).unwrap();
        drop(db);
        let before = std::fs::read(&target).unwrap();
        let evidence = std::env::var_os("ARA_READ_HOST_OUT").map(PathBuf::from)
            .map(|out| out.join(if acp { "sqlite-acp" } else { "sqlite-rpc" }));
        if let Some(out) = &evidence {
            std::fs::create_dir_all(out).unwrap();
            std::fs::write(out.join("database-before.sqlite"), &before).unwrap();
        }
        let up = upstream(vec![calls(&[("sqlite-unicode", "read", json!({"path":"unicode.sqlite:entries?limit=1"}))]), answer("Actual SQLite Unicode receipt inspected.")]).await;
        let mut child;
        let session_file;
        if acp {
            child = RpcChild::spawn_acp(&env, &up, &["--tools", "read", "--edit-mode", "replace"]);
            child.acp_initialize(json!({"fs":{"readTextFile":false,"writeTextFile":false}}));
            let opened = child.acp_new(env.work.path());
            let id = opened["sessionId"].as_str().unwrap();
            child.acp_prompt("sqlite-unicode-run", id);
            assert_eq!(child.acp_success("sqlite-unicode-run")["stopReason"], "end_turn");
            session_file = acp_session_file(&env, id);
            child.join_after_eof(0);
        } else {
            child = RpcChild::spawn(&env, &up, &["--tools", "read", "--edit-mode", "replace"]);
            child.ready();
            child.prompt("sqlite-unicode-run");
            session_file = PathBuf::from(child.state("sqlite-file")["sessionFile"].as_str().unwrap());
            child.finish();
        }
        let entries = journal(&session_file);
        let result = receipt(&entries, "sqlite-unicode");
        assert_eq!(result["isError"], false, "{result}");
        let rendered = text_content(result);
        if let Some(out) = &evidence {
            std::fs::write(out.join("database-after.sqlite"), std::fs::read(&target).unwrap()).unwrap();
            std::fs::write(out.join("journal.json"), serde_json::to_vec_pretty(&entries).unwrap()).unwrap();
            std::fs::write(out.join("tool-receipt.json"), serde_json::to_vec_pretty(result).unwrap()).unwrap();
            std::fs::write(out.join("rendered.txt"), &rendered).unwrap();
            std::fs::write(out.join("host-frames.json"), serde_json::to_vec_pretty(&child.seen).unwrap()).unwrap();
        }
        assert!(rendered.contains("姓名") && rendered.contains(first), "{rendered}");
        assert!(rendered.contains("<BLOB 1.3KB>"), "{rendered}");
        assert!(rendered.contains("9223372036854776000"), "{rendered}");
        assert!(rendered.contains("red"), "{rendered}");
        for line in rendered.lines() {
            assert!(ara_cli::terminal::render_utils::width(line) <= 120, "{line}");
        }
        assert_eq!(result["details"]["meta"]["source"]["value"], target.to_string_lossy().to_string());
        assert_eq!(std::fs::read(&target).unwrap(), before);
        let requests = up.requests.lock().await;
        assert_eq!(requests.len(), 2);
        if let Some(out) = &evidence {
            // Controlled local provider bodies only; no HTTP authorization headers.
            let bodies = requests.iter().map(|request| request["body"].clone()).collect::<Vec<_>>();
            std::fs::write(out.join("provider-bodies.json"), serde_json::to_vec_pretty(&bodies).unwrap()).unwrap();
        }
        assert!(requests[1]["body"]["messages"].as_array().unwrap().iter().any(|message| message["role"] == "tool" && message["tool_call_id"] == "sqlite-unicode"));
    }
}

#[tokio::test(flavor="multi_thread")]
async fn read_column_cap_actual_host_config_rebind_and_acp_bypass(){
    {
        let env=Env::new();let target=env.work.path().join("wide.txt");let wide="x".repeat(192);let original=format!("head\n{wide}\ntail\n");std::fs::write(&target,&original).unwrap();
        std::fs::create_dir_all(env.home.path().join("agent")).unwrap();std::fs::write(env.home.path().join("agent/config.yml"),"tools: {outputMaxColumns: 8}\nlsp: {enabled: false}\n").unwrap();
        std::fs::create_dir_all(env.work.path().join(".ara")).unwrap();let project=env.work.path().join(".ara/config.yml");std::fs::write(&project,"tools.outputMaxColumns: 64\n").unwrap();
        let up=upstream(vec![calls(&[("cap64","read",json!({"path":"wide.txt"}))]),answer("First cap inspected."),calls(&[("cap25","read",json!({"path":"wide.txt"}))]),answer("Retained cap update inspected.")]).await;
        let mut child=RpcChild::spawn(&env,&up,&["--tools","read,edit","--edit-mode","hashline"]);child.ready();child.prompt("cap-first");child.until(|frame|frame["type"]=="agent_end");let first_file=PathBuf::from(child.state("first-file")["sessionFile"].as_str().unwrap());
        let first=journal(&first_file);let result=receipt(&first,"cap64");assert_eq!(result["isError"],false);assert!(text_content(result).contains(&format!("2:{}…","x".repeat(64))));assert!(!text_content(result).contains(&wide));assert_eq!(result["details"]["meta"]["limits"]["columnTruncated"]["maxColumn"],json!(64));
        std::fs::write(&project,"tools: {outputMaxColumns: 2.5}\n").unwrap();child.send(json!({"id":"cap-new","type":"new_session"}));child.success("cap-new");child.prompt("cap-second");child.until(|frame|frame["type"]=="agent_end");let second_file=PathBuf::from(child.state("second-file")["sessionFile"].as_str().unwrap());child.finish();
        let second=journal(&second_file);let result=receipt(&second,"cap25");assert_eq!(result["isError"],false);assert!(text_content(result).contains("2:xx…"));assert_eq!(result["details"]["meta"]["limits"]["columnTruncated"]["maxColumn"],json!(2.5));assert_eq!(std::fs::read_to_string(&target).unwrap(),original);
        let requests=up.requests.lock().await;assert_eq!(requests.len(),4);assert!(requests[1]["body"]["messages"].as_array().unwrap().iter().any(|message|message["role"]=="tool"&&message["tool_call_id"]=="cap64"&&text_content(message).contains(&format!("2:{}…","x".repeat(64)))));
    }
    {
        let env=Env::new();let target=env.work.path().join("editor.txt");std::fs::write(&target,"disk\n").unwrap();std::fs::create_dir_all(env.home.path().join("agent")).unwrap();std::fs::write(env.home.path().join("agent/config.yml"),"tools: {outputMaxColumns: 64}\nlsp: {enabled: false}\n").unwrap();
        let wide="e".repeat(192);let editor=format!("{wide}\n");let up=upstream(vec![calls(&[("uncapped-editor","read",json!({"path":"editor.txt"}))]),answer("Editor early return inspected.")]).await;
        let mut child=RpcChild::spawn_acp(&env,&up,&["--tools","read,edit","--edit-mode","hashline"]);child.acp_initialize(json!({"fs":{"readTextFile":true,"writeTextFile":false}}));let opened=child.acp_new(env.work.path());let id=opened["sessionId"].as_str().unwrap();child.acp_prompt("editor-cap",id);let mut cursor=0;let request=child.peer_request("fs/read_text_file",&mut cursor);assert_eq!(request["params"]["path"],target.to_string_lossy().to_string());child.send(json!({"jsonrpc":"2.0","id":request["id"],"result":{"content":editor}}));assert_eq!(child.acp_success("editor-cap")["stopReason"],"end_turn");child.join_after_eof(0);
        let entries=journal(&acp_session_file(&env,id));let result=receipt(&entries,"uncapped-editor");assert_eq!(result["isError"],false);assert!(text_content(result).contains(&wide));assert!(result["details"]["meta"]["limits"]["columnTruncated"].is_null());assert_eq!(std::fs::read_to_string(&target).unwrap(),"disk\n");
    }
}


#[tokio::test(flavor = "multi_thread")]
async fn read_plan_reference_alias_actual_hosts_group() {
    // Source branches read.ts744-760/1402-1447 and AgentSession's current
    // reference, not an approval marker. Real children; controlled model/peer.
    use ara_cli::host_edit_session::resolve_read_plan_reference;
    let pure = tempfile::tempdir().unwrap();
    let uncreated_root = pure.path().join("uncreated");
    for (input, relative) in [
        ("local://a%2520.md", "a .md"),
        ("local:/a%2520.md", "a%20.md"),
        ("local:///a%2520.md", "a%20.md"),
        ("local:foo", "oo"),
        ("local:", ""),
        ("local:/", ""),
        ("local://folder/a.md?query#fragment", "folder/a.md"),
        ("local://plugin:name/a.md", "plugin:name/a.md"),
    ] {
        assert_eq!(resolve_read_plan_reference(&uncreated_root, input).unwrap(),
            ara_tools::normalize(&uncreated_root.join(relative)), "{input}");
    }
    for input in ["LOCAL://a.md", "file://a.md", "local:/%ZZ", "local://%FF", "local://%2e%2e/a.md", "local://%2fescape.md"] {
        assert!(resolve_read_plan_reference(&uncreated_root, input).is_err(), "{input}");
    }
    assert!(!uncreated_root.exists(), "pure alias resolution cannot initialize its root");
    #[cfg(windows)]
    {
        // Fixed Node path.resolve accepts same-device relative drive paths;
        // ensureWithinRoot's string comparison rejects a changed drive case.
        let upper = Path::new("D:/ara-alias-lexical-only/local");
        let lower = Path::new("d:/ara-alias-lexical-only/local");
        assert_eq!(resolve_read_plan_reference(upper, "local://D:plan.md").unwrap(), upper.join("plan.md"));
        assert_eq!(resolve_read_plan_reference(lower, "local://d:plan.md").unwrap(), lower.join("plan.md"));
        assert!(resolve_read_plan_reference(upper, "local://d:plan.md").is_err());
        assert!(resolve_read_plan_reference(lower, "local://D:plan.md").is_err());
        assert!(resolve_read_plan_reference(upper, "local://E:plan.md").is_err());
        assert!(resolve_read_plan_reference(upper, "local://D:/absolute.md").is_err());
    }

    // A normal RPC Session owns the default local://PLAN.md reference. Exercise
    // priority and a new Session using its actual private Native builtin path.
    {
        let env = acp_env(true);
        let body = "rpc plan first\nrpc plan second\nrpc plan third\n";
        let requested_absolute = env.work.path().join("PLAN.md");
        let up = upstream(vec![
            calls(&[
                ("alias-rpc-draft", "write", json!({"path":"local://PLAN.md","content":body})),
                ("alias-rpc-propose-usage", "read", json!({"path":"xd://propose"})),
                ("alias-rpc-no-proposal", "write", json!({"path":"xd://propose","content":"feature"})),
            ]),
            calls(&[
                ("alias-rpc-line", "read", json!({"path":"PLAN.md:raw:2-2"})),
                ("alias-rpc-absolute", "read", json!({"path":format!("{}:raw:1-1",requested_absolute.display())})),
            ]),
            answer("Normal default plan alias inspected."),
            calls(&[("alias-rpc-real", "read", json!({"path":"PLAN.md:raw"}))]),
            answer("Literal cwd priority inspected."),
            calls(&[("alias-rpc-suffix", "read", json!({"path":"PLAN.md:raw"}))]),
            answer("Unique suffix priority inspected."),
            calls(&[("alias-rpc-stale", "read", json!({"path":"PLAN.md"}))]),
            answer("Disappeared reference receipt inspected."),
            calls(&[("alias-rpc-fresh", "read", json!({"path":"PLAN.md"}))]),
            answer("Fresh Session isolation inspected."),
        ]).await;
        let mut child = RpcChild::spawn(&env, &up, &["--tools", "read,write", "--edit-mode", "replace"]);
        child.ready();
        child.prompt("alias-rpc-initial-run");
        child.until(|frame| frame["type"] == "agent_end");
        let file = PathBuf::from(child.state("alias-rpc-state")["sessionFile"].as_str().unwrap());
        let saved = ara_session::SessionJournal::open(&file).unwrap();
        let artifacts = ara_cli::session_artifacts::SessionArtifacts::for_journal(&saved);
        let local = ara_cli::mcp_native::local::session_root(artifacts.directory(), saved.session_id()).unwrap();
        let target = local.join("PLAN.md");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), body);
        let initial = journal(&file);
        for (call, expected) in [("alias-rpc-line", "rpc plan second"), ("alias-rpc-absolute", "rpc plan first")] {
            let result = receipt(&initial, call);
            assert_eq!(result["isError"], false, "{result}");
            assert_eq!(text_content(result), expected);
            assert_eq!(result["details"]["resolvedPath"], json!(target.to_string_lossy()));
            assert!(result["details"]["suffixResolution"].is_null());
        }
        assert_eq!(text_content(receipt(&initial, "alias-rpc-propose-usage")),
            "Write your plan's <slug> (matching local://<slug>-plan.md) as plain text to xd://propose to submit the plan for approval. Valid only while plan mode is active.");
        let absent = receipt(&initial, "alias-rpc-no-proposal");
        assert_eq!(absent["isError"], true);
        assert_eq!(text_content(absent), "No plan is awaiting approval — xd://propose only accepts a plan title while plan mode is active.");
        std::fs::write(&requested_absolute, "real cwd priority\n").unwrap();
        child.prompt("alias-rpc-real-run");
        child.until(|frame| frame["type"] == "agent_end");
        assert_eq!(text_content(receipt(&journal(&file), "alias-rpc-real")), "real cwd priority\n");
        std::fs::remove_file(&requested_absolute).unwrap();
        let suffix = env.work.path().join("source/PLAN.md");
        std::fs::create_dir_all(suffix.parent().unwrap()).unwrap();
        std::fs::write(&suffix, "suffix priority\n").unwrap();
        child.prompt("alias-rpc-suffix-run");
        child.until(|frame| frame["type"] == "agent_end");
        let suffix_entries = journal(&file);
        let suffix_result = receipt(&suffix_entries, "alias-rpc-suffix");
        assert_eq!(suffix_result["isError"], false);
        assert!(text_content(suffix_result).contains("suffix priority"));
        assert_eq!(suffix_result["details"]["suffixResolution"]["to"], "source/PLAN.md");
        std::fs::remove_file(&suffix).unwrap();
        std::fs::remove_file(&target).unwrap();
        child.prompt("alias-rpc-stale-run");
        child.until(|frame| frame["type"] == "agent_end");
        assert_eq!(receipt(&journal(&file), "alias-rpc-stale")["isError"], true);
        assert!(!target.exists(), "stale alias cannot recreate the plan");
        std::fs::write(&target, body).unwrap();
        child.send(json!({"id":"alias-rpc-new-session","type":"new_session"}));
        child.success("alias-rpc-new-session");
        child.prompt("alias-rpc-fresh-run");
        child.until(|frame| frame["type"] == "agent_end");
        let fresh_file = PathBuf::from(child.state("alias-rpc-fresh-state")["sessionFile"].as_str().unwrap());
        assert_ne!(fresh_file, file);
        assert_eq!(receipt(&journal(&fresh_file), "alias-rpc-fresh")["isError"], true);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), body);
        child.finish();
        assert!(!child.seen.iter().any(|frame| frame["method"] == "elicitation/create"));
        if let Some(output) = std::env::var_os("ARA_READ_HOST_OUT") {
            let directory = PathBuf::from(output).join("plan-reference-rpc");
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join("journal.json"), serde_json::to_vec_pretty(&journal(&file)).unwrap()).unwrap();
            std::fs::write(directory.join("fresh-journal.json"), serde_json::to_vec_pretty(&journal(&fresh_file)).unwrap()).unwrap();
            std::fs::write(directory.join("host-frames.json"), serde_json::to_vec_pretty(&child.seen).unwrap()).unwrap();
            std::fs::write(directory.join("model-requests.json"), serde_json::to_vec_pretty(&*up.requests.lock().await).unwrap()).unwrap();
            std::fs::write(directory.join("fixtures.json"), serde_json::to_vec_pretty(&json!({"defaultPlan":body,
                "localPlanPath":target,"literalPlan":"real cwd priority\n","suffixPlan":"suffix priority\n",
                "lexicalOnlyRoot":uncreated_root,"lexicalOnlyRootExists":uncreated_root.exists()})).unwrap()).unwrap();
            std::fs::write(directory.join("settlement.json"), serde_json::to_vec_pretty(&json!({
                "exitCode":child.child.try_wait().unwrap().unwrap().code(),"served":up.served(),"stderr":*child.stderr.lock().unwrap()})).unwrap()).unwrap();
        }
    }

    // The same Host owner follows an ACP reference after Refine and after
    // Approve, including normal tool refresh. No approved-only flag is supplied.
    {
        let env = acp_env(true);
        let body = "# Feature\nacp plan second\nacp plan third\n";
        let up = upstream(vec![
            calls(&[
                ("alias-acp-draft", "write", json!({"path":"local://feature-plan.md","content":body})),
                ("alias-acp-propose-usage", "read", json!({"path":"xd://propose"})),
                ("alias-acp-resolve-usage", "read", json!({"path":"xd://resolve"})),
                ("alias-acp-reject-usage", "read", json!({"path":"xd://reject"})),
            ]),
            calls(&[("alias-acp-refine", "write", json!({"path":"xd://propose","content":"\u{feff} feature \r\n"}))]),
            calls(&[("alias-acp-refined-read", "read", json!({"path":"feature-plan.md:raw:2-2"}))]),
            calls(&[("alias-acp-approve", "write", json!({"path":"xd://propose","content":"\"feature\""}))]),
            calls(&[("alias-acp-approved-read", "read", json!({"path":"feature-plan.md:raw:1-1"}))]),
            answer("Refined and approved current references inspected."),
            calls(&[("alias-acp-retained-read", "read", json!({"path":"feature-plan.md:raw:3-3"}))]),
            answer("Same Session tool refresh reference inspected."),
            calls(&[("alias-acp-fresh-read", "read", json!({"path":"feature-plan.md"}))]),
            answer("Fresh ACP Session isolation inspected."),
            calls(&[("alias-acp-stale-read", "read", json!({"path":"feature-plan.md"}))]),
            answer("Disappeared ACP current reference inspected."),
        ]).await;
        let mut child = RpcChild::spawn_acp(&env, &up, &["--tools", "read,write", "--edit-mode", "replace"]);
        child.acp_initialize(json!({"fs":{"readTextFile":false,"writeTextFile":false},"elicitation":{"form":{}}}));
        let opened = child.acp_new(env.work.path());
        let id = opened["sessionId"].as_str().unwrap().to_owned();
        child.acp_request("alias-acp-enter-plan", "session/set_mode", json!({"sessionId":id,"modeId":"plan"}));
        child.acp_prompt("alias-acp-main-run", &id);
        let mut cursor = 0;
        for (call, choice) in [("alias-acp-refine", "Refine plan"), ("alias-acp-approve", "Approve and execute")] {
            let request = child.peer_request("elicitation/create", &mut cursor);
            assert_eq!(request["params"]["sessionId"], id);
            child.send(json!({"jsonrpc":"2.0","id":request["id"],"result":{"action":"accept","content":{"value":choice}}}));
            child.until(|frame| terminal_acp_tool(frame, call));
        }
        assert_eq!(child.acp_success("alias-acp-main-run")["stopReason"], "end_turn");
        let file = acp_session_file(&env, &id);
        let initial = journal(&file);
        for (call, title) in [("alias-acp-refine", "feature"), ("alias-acp-approve", "\"feature\"")] {
            let proposal = receipt(&initial, call);
            assert_eq!(proposal["isError"], false);
            assert_eq!(proposal["details"]["xdev"], json!({"tool":"propose","mode":"execute",
                "args":{"title":title},"inner":{"planFilePath":"local://feature-plan.md","title":"feature","planExists":true}}));
        }
        for (call, usage) in [
            ("alias-acp-propose-usage", "Write your plan's <slug> (matching local://<slug>-plan.md) as plain text to xd://propose to submit the plan for approval. Valid only while plan mode is active."),
            ("alias-acp-resolve-usage", "Write a one-sentence reason as plain text to xd://resolve to APPLY the pending staged action (e.g. a tool preview)."),
            ("alias-acp-reject-usage", "Write a one-sentence reason as plain text to xd://reject to DISCARD the pending staged action (e.g. a tool preview)."),
        ] {
            let output = receipt(&initial, call);
            assert_eq!(output["isError"], false);
            assert_eq!(text_content(output), usage);
        }
        let target = PathBuf::from(receipt(&initial, "alias-acp-refined-read")["details"]["resolvedPath"].as_str().unwrap());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), body);
        for (call, expected) in [("alias-acp-refined-read", "acp plan second"), ("alias-acp-approved-read", "# Feature")] {
            let result = receipt(&initial, call);
            assert_eq!(result["isError"], false, "{result}");
            assert_eq!(text_content(result), expected);
            assert_eq!(result["details"]["resolvedPath"], json!(target.to_string_lossy()));
            assert!(result["details"]["suffixResolution"].is_null());
        }
        let loaded = child.acp_request("alias-acp-retained-load", "session/load",
            json!({"sessionId":id,"cwd":env.work.path(),"mcpServers":[]}));
        assert_eq!(loaded["modes"]["currentModeId"], "default");
        child.acp_prompt("alias-acp-retained-run", &id);
        child.acp_success("alias-acp-retained-run");
        assert_eq!(text_content(receipt(&journal(&file), "alias-acp-retained-read")), "acp plan third");
        let fresh = child.acp_request("alias-acp-fresh-open", "session/new", json!({"cwd":env.work.path(),"mcpServers":[]}));
        let fresh_id = fresh["sessionId"].as_str().unwrap();
        assert_ne!(fresh_id, id);
        child.acp_prompt("alias-acp-fresh-run", fresh_id);
        child.acp_success("alias-acp-fresh-run");
        let fresh_file = acp_session_file(&env, fresh_id);
        assert_eq!(receipt(&journal(&fresh_file), "alias-acp-fresh-read")["isError"], true);
        std::fs::remove_file(&target).unwrap();
        child.acp_prompt("alias-acp-stale-run", &id);
        child.acp_success("alias-acp-stale-run");
        assert_eq!(receipt(&journal(&file), "alias-acp-stale-read")["isError"], true);
        assert!(!target.exists());
        child.join_after_eof(0);
        assert_eq!(child.seen.iter().filter(|frame| frame["method"] == "elicitation/create").count(), 2,
            "READ cannot initiate another approval");
        assert!(!child.seen.iter().any(|frame| frame["method"] == "fs/write_text_file"));
        if let Some(output) = std::env::var_os("ARA_READ_HOST_OUT") {
            let directory = PathBuf::from(output).join("plan-reference-acp");
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join("journal.json"), serde_json::to_vec_pretty(&journal(&file)).unwrap()).unwrap();
            std::fs::write(directory.join("fresh-journal.json"), serde_json::to_vec_pretty(&journal(&fresh_file)).unwrap()).unwrap();
            std::fs::write(directory.join("host-frames.json"), serde_json::to_vec_pretty(&child.seen).unwrap()).unwrap();
            std::fs::write(directory.join("model-requests.json"), serde_json::to_vec_pretty(&*up.requests.lock().await).unwrap()).unwrap();
            std::fs::write(directory.join("fixtures.json"), serde_json::to_vec_pretty(&json!({"planBody":body,"localPlanPath":target,
                "planExistsAfterStaleRead":target.exists()})).unwrap()).unwrap();
            std::fs::write(directory.join("settlement.json"), serde_json::to_vec_pretty(&json!({
                "exitCode":child.child.try_wait().unwrap().unwrap().code(),"served":up.served(),"stderr":*child.stderr.lock().unwrap()})).unwrap()).unwrap();
        }
    }
}


// b91db66e ACP autosave delta. These rows use the existing real ara child,
// real proposal Host and peer approval route; the old response banks above are
// unchanged. Configuration is published after Session registration and, for
// form rows, while its matching approval request is pending.
fn acp_autosave_settings(env: &Env, autosave: &str, directory: Option<&str>) {
    let mut source =
        "lsp:\n  enabled: false\nedit:\n  enforceSeenLines: false\nplan:\n  enabled: true\n"
            .to_owned();
    if !autosave.is_empty() {
        source.push_str(&format!("  autosave: {autosave}\n"));
    }
    if let Some(directory) = directory {
        source.push_str(&format!(
            "  autosaveDir: {}\n",
            serde_json::to_string(directory).unwrap()
        ));
    }
    std::fs::write(env.home.path().join("agent/config.yml"), source).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn acp_plan_autosave_upgrade_complete_host_matrix() {
    for case in [
        "default",
        "disabled",
        "strict-string",
        "strict-number",
        "enabled",
        "empty",
        "blank-dir",
        "nonstring-dir",
        "relative-dir",
        "absolute-dir",
        "tilde-dir",
        "tilde-prefix",
        "collision",
        "failure",
        "project-disabled",
        "project-replaced",
        "refresh-error",
    ] {
        let env = acp_env(true);
        let default_dir = env.work.path().join(".ara/plans");
        let custom_dir = env.work.path().join("never-model-autosave-destination");
        let home_dir = env.home.path().join("saved-plans");
        let mut save_dir = default_dir.clone();
        let mut configured_dir = None::<String>;
        let title = match case {
            "relative-dir" => "Split PyEnvironmentBackend request into PyO3 methods",
            "blank-dir" => "Plan",
            "tilde-dir" => "ABCDEFGHIJKLMNOPQRSTUVWXYZABCDEFGHIJKLMN",
            "absolute-dir" => "auth storage plan",
            _ => "auth storage",
        };
        let filename = match case {
            "relative-dir" => "SPLIT_PYENVIRONMENTBACKEND_PLAN.md",
            "blank-dir" => "PLAN.md",
            "tilde-dir" => "ABCDEFGHIJKLMNOPQRSTUVWXYZABCDEF_PLAN.md",
            _ => "AUTH_STORAGE_PLAN.md",
        };
        let body = if case == "empty" {
            "\u{feff} \t\n\u{3000}".to_owned()
        } else {
            format!("# Approved {case}\n\nPreserve this exact plan copy.\n")
        };
        match case {
            "blank-dir" => configured_dir = Some("\u{feff} \u{3000}".into()),
            "relative-dir" => {
                configured_dir = Some(" unused/../never-model-autosave-destination ".into());
                save_dir = custom_dir.clone();
            }
            "absolute-dir" | "collision" | "failure" => {
                configured_dir = Some(custom_dir.to_string_lossy().into_owned());
                save_dir = custom_dir.clone();
            }
            "tilde-dir" => {
                configured_dir = Some("~/saved-plans".into());
                save_dir = home_dir.clone();
            }
            "tilde-prefix" => {
                configured_dir = Some("~saved-plans".into());
                save_dir = home_dir.clone();
            }
            _ => {}
        }
        if case == "collision" {
            std::fs::create_dir_all(&save_dir).unwrap();
            std::fs::write(save_dir.join(filename), "preexisting original\n").unwrap();
            std::fs::write(
                save_dir.join("AUTH_STORAGE_PLAN-1.md"),
                "preexisting suffix\n",
            )
            .unwrap();
        } else if case == "failure" {
            // mkdir fails before an exclusive write; the approved reference and
            // mode transition must still survive this Host filesystem failure.
            std::fs::write(&save_dir, "blocking directory creation\n").unwrap();
        }
        let up = upstream(vec![
            calls(&[(
                "autosave-draft",
                "write",
                json!({"path":"local://auth-storage-plan.md","content":body}),
            )]),
            calls(&[(
                "autosave-propose",
                "write",
                json!({"path":"xd://propose","content":title}),
            )]),
            answer("Known approval result inspected."),
            answer("Same Session approved reference inspected."),
        ])
        .await;
        let mut child = RpcChild::spawn_acp(&env, &up, &["--tools", "read"]);
        let form = case != "nonstring-dir";
        child.acp_initialize(if form {
            json!({"elicitation":{"form":{}}})
        } else {
            json!({})
        });
        let opened = child.acp_new(env.work.path());
        let id = opened["sessionId"].as_str().unwrap().to_owned();
        child.acp_request(
            "autosave-enter",
            "session/set_mode",
            json!({"sessionId":id,"modeId":"plan"}),
        );
        let configure = || {
            let setting = match case {
                "default" => "",
                "disabled" => "false",
                "strict-string" => "\"true\"",
                "strict-number" => "1",
                _ => "true",
            };
            acp_autosave_settings(&env, setting, configured_dir.as_deref());
            if case == "nonstring-dir" {
                let config = env.home.path().join("agent/config.yml");
                let source = std::fs::read_to_string(&config).unwrap();
                std::fs::write(config, format!("{source}  autosaveDir: 42\n")).unwrap();
            } else if matches!(case, "project-disabled" | "project-replaced") {
                std::fs::create_dir_all(env.work.path().join(".ara")).unwrap();
                std::fs::write(
                    env.work.path().join(".ara/config.yml"),
                    if case == "project-disabled" {
                        "plan:\n  autosave: false\n"
                    } else {
                        "plan: null\n"
                    },
                )
                .unwrap();
            } else if case == "refresh-error" {
                // Approval admission has already happened, but Native refresh
                // rereads this same global file before autosave can run. This
                // is the real Host refresh failure, not a getter exception.
                std::fs::write(env.home.path().join("agent/config.yml"), "plan: [\n").unwrap();
            }
        };
        if !form {
            configure();
        }
        child.acp_prompt("autosave-run", &id);
        if form {
            let mut cursor = 0;
            let request = child.peer_request("elicitation/create", &mut cursor);
            assert_eq!(request["params"]["sessionId"], id);
            assert!(!save_dir.join(filename).is_file() || case == "collision");
            configure();
            child.send(json!({"jsonrpc":"2.0","id":request["id"],"result":{
                "action":"accept","content":{"value":"Approve and execute"}
            }}));
        }
        assert_eq!(
            child.acp_success("autosave-run")["stopReason"],
            "end_turn",
            "{case}"
        );
        let output = child.acp_result("autosave-propose");
        if case == "refresh-error" {
            // Do not let the shared healthy-approval assertions below turn an
            // earlier Native refresh fault into autosave getter coverage.
            assert_eq!(output["isError"], true, "{case}: {output}");
            assert_eq!(output["details"]["xdev"]["inner"]["planFilePath"], "local://auth-storage-plan.md");
            assert_eq!(output["details"]["xdev"]["inner"]["planExists"], true);
            assert_eq!(output["details"]["xdev"]["inner"]["title"], "auth-storage");
            let failure = text_content(&output);
            assert!(failure.starts_with("Plan approval was received, but the mode transition failed; plan mode remains active at local://auth-storage-plan.md. Tool refresh failed: "), "{failure}");
            assert!(failure.contains(&format!("parsing native settings {}", env.home.path().join("agent").join("config.yml").display())), "{failure}");
            assert!(!failure.contains("Plan approved at"));
            assert!(!failure.contains("Plan autosave failed; continuing."));
            assert!(!default_dir.exists());
            assert!(!custom_dir.exists());
            assert_eq!(std::fs::read_to_string(env.home.path().join("agent/config.yml")).unwrap(), "plan: [\n");
            // Restore settings only. No proposal replay, mode exit, new Session,
            // or conversation replacement is used to recover this same owner.
            acp_autosave_settings(&env, "true", None);
            let mode = child.acp_request("autosave-mode", "session/resume", json!({"sessionId":id,"cwd":env.work.path(),"mcpServers":[]}));
            assert_eq!(mode["modes"]["currentModeId"], "plan");
            child.acp_prompt("autosave-reference", &id);
            assert_eq!(child.acp_success("autosave-reference")["stopReason"], "end_turn");
            child.join_after_eof(0);
            let file = acp_session_file(&env, &id);
            let entries = journal(&file);
            assert_eq!(receipt(&entries, "autosave-propose")["isError"], true);
            assert_eq!(text_content(receipt(&entries, "autosave-propose")), failure);
            let saved = ara_session::SessionJournal::open(&file).unwrap();
            let artifacts = ara_cli::session_artifacts::SessionArtifacts::for_journal(&saved);
            let local = ara_cli::mcp_native::local::session_root(artifacts.directory(), saved.session_id()).unwrap();
            assert_eq!(std::fs::read_to_string(local.join("auth-storage-plan.md")).unwrap(), body);
            assert!(!default_dir.exists(), "failed approval must not autosave later during reference preparation");
            assert!(!custom_dir.exists());
            assert!(!child.seen.iter().any(|frame| frame["method"] == "fs/write_text_file"));
            assert_eq!(child.seen.iter().filter(|frame| frame["method"] == "elicitation/create").count(), 1);
            assert!(!child.stderr.lock().unwrap().contains("Failed to autosave approved plan"), "refresh failure occurs before autosave");
            let requests = up.requests.lock().await;
            assert_eq!(requests.len(), 4);
            assert!(has_model_tool(&requests[2], "write"), "failed refresh retains plan tool authority");
            assert!(has_model_tool(&requests[3], "write"), "same Session remains in plan mode after restoring settings");
            assert!(!requests[3]["body"]["messages"].to_string().contains("Approved plan inlined below"), "failed approval must not publish approved reference history");
            eprintln!("ARA-ACP AUTOSAVE b91db66e PASS refresh-error");
            continue;
        }
        assert_ne!(output["isError"], true, "{case}: {output}");
        assert_eq!(
            output["details"]["xdev"]["inner"]["planFilePath"],
            "local://auth-storage-plan.md"
        );
        assert_eq!(output["details"]["xdev"]["inner"]["planExists"], true);
        assert!(
            output["details"]["xdev"]["inner"]["title"]
                .as_str()
                .unwrap()
                .is_ascii()
        );
        let warning = if case == "failure" {
            " (Plan autosave failed; continuing.)"
        } else {
            ""
        };
        assert_eq!(
            text_content(&output),
            format!(
                "Plan approved at local://auth-storage-plan.md. Plan mode exited; proceed with the implementation.{warning}"
            ),
            "{case}"
        );
        assert!(
            !output
                .to_string()
                .contains("never-model-autosave-destination"),
            "saved destination is not a model tool detail"
        );
        let mode = child.acp_request(
            "autosave-mode",
            "session/resume",
            json!({
                "sessionId":id,"cwd":env.work.path(),"mcpServers":[]
            }),
        );
        assert_eq!(mode["modes"]["currentModeId"], "default", "{case}");
        let enabled = !matches!(
            case,
            "default"
                | "disabled"
                | "strict-string"
                | "strict-number"
                | "project-disabled"
                | "project-replaced"
                | "refresh-error"
        );
        if enabled && !matches!(case, "failure" | "empty") {
            let name = if case == "collision" {
                "AUTH_STORAGE_PLAN-2.md"
            } else {
                filename
            };
            assert_eq!(
                std::fs::read_to_string(save_dir.join(name)).unwrap(),
                body,
                "{case}"
            );
            if case == "collision" {
                assert_eq!(
                    std::fs::read_to_string(save_dir.join(filename)).unwrap(),
                    "preexisting original\n"
                );
                assert_eq!(
                    std::fs::read_to_string(save_dir.join("AUTH_STORAGE_PLAN-1.md")).unwrap(),
                    "preexisting suffix\n"
                );
                assert_eq!(std::fs::read_dir(&save_dir).unwrap().count(), 3);
            } else {
                assert_eq!(std::fs::read_dir(&save_dir).unwrap().count(), 1, "{case}");
            }
        } else if case == "failure" {
            assert_eq!(
                std::fs::read_to_string(&save_dir).unwrap(),
                "blocking directory creation\n"
            );
        } else {
            assert!(
                !default_dir.exists(),
                "disabled autosave creates no default destination: {case}"
            );
        }
        if save_dir != default_dir {
            assert!(!default_dir.exists(), "{case}");
        }
        child.acp_prompt("autosave-reference", &id);
        assert_eq!(
            child.acp_success("autosave-reference")["stopReason"],
            "end_turn"
        );
        child.join_after_eof(0);
        let entries = journal(&acp_session_file(&env, &id));
        assert_eq!(
            receipt(&entries, "autosave-propose")["isError"],
            false,
            "{case}"
        );
        assert!(
            !child
                .seen
                .iter()
                .any(|frame| frame["method"] == "fs/write_text_file")
        );
        assert_eq!(
            child
                .seen
                .iter()
                .filter(|frame| frame["method"] == "elicitation/create")
                .count(),
            usize::from(form)
        );
        let requests = up.requests.lock().await;
        assert_eq!(requests.len(), 4, "{case}");
        assert_eq!(
            requests[3]["body"]["messages"]
                .to_string()
                .contains("Approved plan inlined below"),
            case != "empty",
            "{case}"
        );
        if case != "empty" {
            assert!(
                requests[3]["body"]["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|message| text_content(message).contains(&body)),
                "{case}"
            );
        }
        assert!(
            !has_model_tool(&requests[2], "write"),
            "mode publication follows known approval even after save failure"
        );
        if case == "failure" {
            assert!(
                child
                    .stderr
                    .lock()
                    .unwrap()
                    .contains("Failed to autosave approved plan")
            );
        }
        eprintln!("ARA-ACP AUTOSAVE b91db66e PASS {case}");
    }

    // Two independently owned native Sessions approve same-titled different
    // bodies in the same project. Replies are released without waiting for the
    // first save, so the filesystem claim owns collision handling across Hosts.
    let first_env = acp_env(true);
    let second_env = acp_env(true);
    let first_body = "# Race first\nPreserve first Session body.\n";
    let second_body = "# Race second\nPreserve second Session body.\n";
    let first_up = upstream(vec![
        calls(&[(
            "race-draft",
            "write",
            json!({"path":"local://race-plan.md","content":first_body}),
        )]),
        calls(&[(
            "race-propose",
            "write",
            json!({"path":"xd://propose","content":"race"}),
        )]),
        answer("First approval preserved."),
    ])
    .await;
    let second_up = upstream(vec![
        calls(&[(
            "race-draft",
            "write",
            json!({"path":"local://race-plan.md","content":second_body}),
        )]),
        calls(&[(
            "race-propose",
            "write",
            json!({"path":"xd://propose","content":"race"}),
        )]),
        answer("Second approval preserved."),
    ])
    .await;
    let mut first_child = RpcChild::spawn_acp(&first_env, &first_up, &["--tools", "read"]);
    let mut second_child = RpcChild::spawn_acp(&second_env, &second_up, &["--tools", "read"]);
    first_child.acp_initialize(json!({"elicitation":{"form":{}}}));
    second_child.acp_initialize(json!({"elicitation":{"form":{}}}));
    let first_opened = first_child.acp_new(first_env.work.path());
    let second_opened = second_child.acp_new(first_env.work.path());
    let first_id = first_opened["sessionId"].as_str().unwrap();
    let second_id = second_opened["sessionId"].as_str().unwrap();
    assert_ne!(first_id, second_id);
    first_child.acp_request(
        "race-enter",
        "session/set_mode",
        json!({"sessionId":first_id,"modeId":"plan"}),
    );
    second_child.acp_request(
        "race-enter",
        "session/set_mode",
        json!({"sessionId":second_id,"modeId":"plan"}),
    );
    first_child.acp_prompt("race-run", first_id);
    second_child.acp_prompt("race-run", second_id);
    let first_request = first_child.peer_request("elicitation/create", &mut 0);
    let second_request = second_child.peer_request("elicitation/create", &mut 0);
    acp_autosave_settings(&first_env, "true", None);
    acp_autosave_settings(&second_env, "true", None);
    let approval = json!({"action":"accept","content":{"value":"Approve and execute"}});
    first_child.send(json!({"jsonrpc":"2.0","id":first_request["id"],"result":approval}));
    second_child.send(json!({"jsonrpc":"2.0","id":second_request["id"],"result":approval}));
    assert_eq!(
        first_child.acp_success("race-run")["stopReason"],
        "end_turn"
    );
    assert_eq!(
        second_child.acp_success("race-run")["stopReason"],
        "end_turn"
    );
    assert_ne!(first_child.acp_result("race-propose")["isError"], true);
    assert_ne!(second_child.acp_result("race-propose")["isError"], true);
    first_child.join_after_eof(0);
    second_child.join_after_eof(0);
    assert_eq!(first_up.served(), 3);
    assert_eq!(second_up.served(), 3);
    let directory = first_env.work.path().join(".ara/plans");
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 2);
    let mut copies = [
        std::fs::read_to_string(directory.join("RACE_PLAN.md")).unwrap(),
        std::fs::read_to_string(directory.join("RACE_PLAN-1.md")).unwrap(),
    ];
    copies.sort();
    let mut expected = [first_body.to_owned(), second_body.to_owned()];
    expected.sort();
    assert_eq!(copies, expected);
    eprintln!("ARA-ACP AUTOSAVE b91db66e PASS cross-session-exclusive");

    // Refine, dismissal and unknown action remain in plan mode and save nothing.
    // Only the final known approval claims a file; live settings turn on while
    // the first matching approval is pending, not at Session registration.
    let env = acp_env(true);
    let body = "# Decisions\nKeep the selected plan unchanged.\n";
    let up = upstream(vec![
        calls(&[(
            "autosave-decisions-draft",
            "write",
            json!({"path":"local://decisions-plan.md","content":body}),
        )]),
        calls(&[(
            "autosave-refine",
            "write",
            json!({"path":"xd://propose","content":"decisions"}),
        )]),
        calls(&[(
            "autosave-dismiss",
            "write",
            json!({"path":"xd://propose","content":"decisions"}),
        )]),
        calls(&[(
            "autosave-unknown",
            "write",
            json!({"path":"xd://propose","content":"decisions"}),
        )]),
        calls(&[(
            "autosave-known",
            "write",
            json!({"path":"xd://propose","content":"decisions"}),
        )]),
        answer("All decisions and known final approval inspected."),
    ])
    .await;
    let mut child = RpcChild::spawn_acp(&env, &up, &["--tools", "read"]);
    child.acp_initialize(json!({"elicitation":{"form":{}}}));
    let opened = child.acp_new(env.work.path());
    let id = opened["sessionId"].as_str().unwrap();
    child.acp_request(
        "autosave-decisions-enter",
        "session/set_mode",
        json!({"sessionId":id,"modeId":"plan"}),
    );
    child.acp_prompt("autosave-decisions-run", id);
    let mut cursor = 0;
    for (call, choice, approved) in [
        (
            "autosave-refine",
            json!({"action":"accept","content":{"value":"Refine plan"}}),
            false,
        ),
        ("autosave-dismiss", json!({"action":"cancel"}), false),
        (
            "autosave-unknown",
            json!({"action":"accept","content":{"value":"unknown choice"}}),
            false,
        ),
        (
            "autosave-known",
            json!({"action":"accept","content":{"value":"Approve and execute"}}),
            true,
        ),
    ] {
        let request = child.peer_request("elicitation/create", &mut cursor);
        assert!(
            !env.work.path().join(".ara/plans").exists(),
            "no save before known approval"
        );
        acp_autosave_settings(&env, "true", None);
        child.send(json!({"jsonrpc":"2.0","id":request["id"],"result":choice}));
        child.until(|frame| terminal_acp_tool(frame, call));
        let output = child.acp_result(call);
        assert_ne!(output["isError"], true);
        assert_eq!(text_content(&output).contains("Plan approved at"), approved);
        if !approved {
            assert!(!env.work.path().join(".ara/plans").exists());
            let mode = child.acp_request(
                &format!("autosave-mode-{call}"),
                "session/resume",
                json!({
                    "sessionId":id,"cwd":env.work.path(),"mcpServers":[]
                }),
            );
            assert_eq!(mode["modes"]["currentModeId"], "plan");
        }
        eprintln!("ARA-ACP AUTOSAVE b91db66e DECISION PASS {call}");
    }
    assert_eq!(
        child.acp_success("autosave-decisions-run")["stopReason"],
        "end_turn"
    );
    child.join_after_eof(0);
    assert_eq!(up.served(), 6);
    assert_eq!(
        std::fs::read_to_string(env.work.path().join(".ara/plans/DECISIONS_PLAN.md")).unwrap(),
        body
    );
    assert_eq!(
        std::fs::read_dir(env.work.path().join(".ara/plans"))
            .unwrap()
            .count(),
        1
    );
}

// b91db66e router203-215,302-305,351-360 and plan-files11-22.
#[tokio::test]
async fn plan_uri_capability_composition_group() {
    use ara_agent::ToolError;
    use ara_tools::{ContentUriPort,ContentUriRoute,UriResource,UriFileResource};
    use tokio_util::sync::CancellationToken;
    struct Cap {prefix:&'static str,route:ContentUriRoute,alias:bool,path:PathBuf,calls:std::sync::atomic::AtomicUsize}
    #[async_trait::async_trait]
    impl ContentUriPort for Cap {
        fn route(&self,url:&str)->ContentUriRoute {if url.starts_with(self.prefix) {self.route}else {ContentUriRoute::Unregistered}}
        fn normalize_url(&self,input:&str)->String {if self.alias {ara_tools::internal_urls::normalize_native_local_alias(input)}else {input.into()}}
        fn locate_sync(&self,_:&str)->Result<Option<PathBuf>,ToolError> {self.calls.fetch_add(1,std::sync::atomic::Ordering::SeqCst);Ok(if self.route==ContentUriRoute::Removed {None}else {Some(self.path.clone())})}
        async fn read_file(&self,_:&str,_:CancellationToken)->Result<Option<UriFileResource>,ToolError>{
            self.calls.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
            Ok(if self.route==ContentUriRoute::Removed {None}else {Some(UriFileResource{path:self.path.clone(),content_type:"text/plain".into(),max_inline_bytes:None,immutable:false})})
        }
        async fn read(&self,_:&str,_:CancellationToken)->Result<UriResource,ToolError>{panic!("no materialization")}
        async fn write(&self,_:&str,_:&str,_:CancellationToken)->Result<(),ToolError>{panic!("no write")}
    }
    tokio::time::timeout(Duration::from_secs(5),async {
        let dir=tempfile::tempdir().unwrap();let first_path=dir.path().join("first.md");let second_path=dir.path().join("second.md");
        std::fs::write(&first_path,"first").unwrap();std::fs::write(&second_path,"second").unwrap();
        for route in [ContentUriRoute::Registered{writable:false},ContentUriRoute::Removed] {
            let first=Arc::new(Cap{prefix:"local://",route,alias:false,path:first_path.clone(),calls:0.into()});
            let second=Arc::new(Cap{prefix:"local://",route:ContentUriRoute::Registered{writable:false},alias:true,path:second_path.clone(),calls:0.into()});
            let chain=ara_cli::host_vault::compose_ports(first.clone(),Some(second.clone()));
            for input in ["local:/x.md","/repo/local://x.md"] {
                assert_eq!(chain.normalize_url(input),input);
                assert_eq!(ara_tools::internal_urls::normalize_plan_uri(Some(chain.as_ref()),input),input);
            }
            let expected=if route==ContentUriRoute::Removed {None}else {Some(first_path.clone())};
            assert_eq!(chain.locate_sync("local://x.md").unwrap(),expected);
            assert_eq!(chain.search_path_only("local://x.md",CancellationToken::new()).await.unwrap(),expected);
            assert_eq!(first.calls.load(std::sync::atomic::Ordering::SeqCst),2);assert_eq!(second.calls.load(std::sync::atomic::Ordering::SeqCst),0);
        }
        let first=Arc::new(Cap{prefix:"local://",route:ContentUriRoute::Registered{writable:false},alias:true,path:first_path.clone(),calls:0.into()});
        let second=Arc::new(Cap{prefix:"local://",route:ContentUriRoute::Registered{writable:false},alias:true,path:second_path.clone(),calls:0.into()});
        let chain=ara_cli::host_vault::compose_ports(first.clone(),Some(second.clone()));assert_eq!(chain.normalize_url("local:/x.md"),"local://x.md");
        assert_eq!(chain.locate_sync(&chain.normalize_url("local:/x.md")).unwrap(),Some(first_path.clone()));assert_eq!(second.calls.load(std::sync::atomic::Ordering::SeqCst),0);
        let no_first=Arc::new(Cap{prefix:"other://",route:ContentUriRoute::Registered{writable:false},alias:false,path:first_path.clone(),calls:0.into()});
        let chain=ara_cli::host_vault::compose_ports(no_first,Some(second));assert_eq!(chain.normalize_url("LOCAL:/x.md"),"LOCAL://x.md");
        eprintln!("ARA-PLAN-URI PASS raw-canonical-first-authority-and-case");
        let journal=ara_session::SessionJournal::create(dir.path(),dir.path()).unwrap();
        let store=ara_cli::session_artifacts::SessionArtifacts::for_journal(&journal);
        let id=format!("artifact://{}",store.save("artifact bytes","fixture").await.unwrap());
        let router=ara_cli::session_artifacts::ArtifactUriRouter::new(store.clone(),None);
        assert!(router.locate_sync(&id).unwrap().is_none());
        assert!(router.search_path_only(&id,CancellationToken::new()).await.unwrap().is_some());
        for route in [ContentUriRoute::Registered{writable:false},ContentUriRoute::Removed] {
            let fallback=Arc::new(Cap{prefix:"artifact://",route,alias:false,path:first_path.clone(),calls:0.into()});
            let router=ara_cli::session_artifacts::ArtifactUriRouter::new(store.clone(),Some(fallback.clone()));
            assert_eq!(router.route(&id),route);
            let expected=if route==ContentUriRoute::Removed {None}else {Some(first_path.clone())};
            assert_eq!(router.locate_sync(&id).unwrap(),expected);
            assert_eq!(router.search_path_only(&id,CancellationToken::new()).await.unwrap(),expected);
            assert_eq!(fallback.calls.load(std::sync::atomic::Ordering::SeqCst),2);
        }
        eprintln!("ARA-PLAN-URI PASS artifact-no-sync-and-fallback-authority");
    }).await.expect("bounded URI composition");
}
