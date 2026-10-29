use ara_testkit::{
    FakeUpstream, Script,
    chunks::{done, finish, text, tool_call},
};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
};

const CHILD_A: &str = "G3_CHILD_A_NATIVE_ONLY";
const CHILD_B: &str = "G3_CHILD_B_NATIVE_ONLY";
const BOUND: Duration = Duration::from_secs(90);

fn call(id: &str, name: &str, args: Value) -> Value {
    json!({"events":[tool_call(0,id,name,&args.to_string()),finish("tool_calls"),done()]})
}
fn answer(value: &str) -> Value {
    json!({"events":[text(value),finish("stop"),done()]})
}
fn child_script(file: &str, body: &str, bash: bool) -> Vec<Value> {
    let mut events = vec![tool_call(
        0,
        "g3-child-write",
        "write",
        &json!({"path":file,"content":body}).to_string(),
    )];
    if bash {
        events.push(tool_call(
            1,
            "g3-child-bash",
            "bash",
            &json!({"command":"echo native-bash-proof > bash-proof.txt"}).to_string(),
        ));
    }
    events.extend([finish("tool_calls"), done()]);
    vec![
        json!({"events":events}),
        answer("Child native work completed."),
    ]
}
fn task_args(agent: &str, assignment: &str) -> Value {
    json!({"agent":agent,"task":assignment,"solutionSpace":"Write only the assigned fixture outputs in your isolated cwd.","isolated":true})
}

/// The router only forwards real HTTP bytes to controlled upstreams. It never builds a Task result.
/// Separate routes avoid relying on parent/child request interleavings in the async scheduler.
struct RoutedUpstream {
    root: FakeUpstream,
    a: FakeUpstream,
    b: FakeUpstream,
    address: SocketAddr,
    router: tokio::task::JoinHandle<()>,
}
impl Drop for RoutedUpstream {
    fn drop(&mut self) {
        self.router.abort();
    }
}
impl RoutedUpstream {
    async fn start(directory: &Path, root: Vec<Value>, a: Vec<Value>, b: Vec<Value>) -> Self {
        Self::start_gated(directory, root, a, b, false).await
    }
    async fn start_gated(
        directory: &Path,
        root: Vec<Value>,
        a: Vec<Value>,
        b: Vec<Value>,
        hold_for_wait: bool,
    ) -> Self {
        async fn upstream(directory: &Path, name: &str, responses: Vec<Value>) -> FakeUpstream {
            FakeUpstream::start(
                serde_json::from_value::<Script>(json!({"responses":responses})).unwrap(),
                Some(directory.join(format!("{name}-requests.jsonl"))),
            )
            .await
            .unwrap()
        }
        let root = upstream(directory, "root", root).await;
        let a = upstream(directory, "child-a", a).await;
        let b = upstream(directory, "child-b", b).await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let destinations = [root.addr, a.addr, b.addr];
        let receipts = directory.join("routed-requests.jsonl");
        let evidence = directory.to_path_buf();
        let router = tokio::spawn(async move {
            loop {
                let Ok((mut client, _)) = listener.accept().await else {
                    break;
                };
                let receipts = receipts.clone();
                let evidence = evidence.clone();
                tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    let mut buffer = [0u8; 8192];
                    let header_end;
                    loop {
                        let count = client.read(&mut buffer).await.unwrap();
                        if count == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&buffer[..count]);
                        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                            header_end = end + 4;
                            break;
                        }
                        assert!(bytes.len() < 256 * 1024, "controlled HTTP header bound");
                    }
                    let headers = String::from_utf8_lossy(&bytes[..header_end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.split_once(':')
                                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                        })
                        .expect("actual finite model request content length");
                    assert!(length < 32 * 1024 * 1024, "controlled model body bound");
                    while bytes.len() < header_end + length {
                        let count = client.read(&mut buffer).await.unwrap();
                        assert!(count > 0);
                        bytes.extend_from_slice(&buffer[..count]);
                    }
                    let body: Value =
                        serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                    let system = body["messages"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|row| row["role"] == "system")
                        .map(|row| row["content"].to_string())
                        .collect::<Vec<_>>()
                        .join("\n");
                    let route = if system.contains(CHILD_A)
                        || system.contains("Investigate the codebase rapidly.")
                        || system.contains("Find bugs author wants fixed before merge.")
                        || system.contains("Review assigned repository scope only.")
                    {
                        1
                    } else if system.contains(CHILD_B) {
                        2
                    } else {
                        0
                    };
                    let mut file = fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(receipts)
                        .unwrap();
                    writeln!(
                        file,
                        "{}",
                        json!({"route":(["root","child-a","child-b"][route]),"body":body})
                    )
                    .unwrap();
                    let mut server = TcpStream::connect(destinations[route]).await.unwrap();
                    server.write_all(&bytes).await.unwrap();
                    let mut response = Vec::new();
                    server.read_to_end(&mut response).await.unwrap();
                    if hold_for_wait && route != 0 {
                        tokio::time::timeout(Duration::from_secs(25),async {loop {
                        let events=lines(&evidence.join("root.events.jsonl"));
                        if events.iter().any(|event|event["type"]=="tool_execution_update"&&event["toolCallId"]=="g3-actual-wait"&&event["partialResult"]["details"]["jobs"].as_array().is_some_and(|jobs|jobs.iter().any(|job|job["kind"]=="task"&&job["status"]=="running"))) {
                            fs::write(evidence.join("wait-observed-original-jobs.json"),serde_json::to_vec_pretty(&events).unwrap()).unwrap();break;
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }}).await.expect("actual Native Wait did not watch original jobs; original model body remains held");
                    }
                    client.write_all(&response).await.unwrap();
                    let _ = client.shutdown().await;
                });
            }
        });
        Self {
            root,
            a,
            b,
            address,
            router,
        }
    }
    fn url(&self) -> String {
        format!("http://{}/v1", self.address)
    }
    fn counts(&self) -> [usize; 3] {
        [self.root.served(), self.a.served(), self.b.served()]
    }
}

struct Bank {
    directory: PathBuf,
    home: PathBuf,
    project: PathBuf,
    initial_head: String,
    initial_index: Vec<u8>,
    keep_alive: bool,
    parent_tools: String,
}
impl Bank {
    fn new(
        label: &str,
        merge: &str,
        apply: bool,
        keep: bool,
        asynchronous: bool,
        batch: bool,
    ) -> Self {
        let base = PathBuf::from(
            std::env::var_os("ARA_TEST_PEER_ARTIFACT_DIR")
                .expect("Root binds the D-only ordinary Task evidence directory"),
        );
        let directory = base.join(format!("{label}-{}", uuid::Uuid::now_v7()));
        let home = directory.join("home");
        let project = directory.join("project");
        fs::create_dir_all(home.join("agent")).unwrap();
        fs::create_dir_all(project.join(".ara/agents")).unwrap();
        fs::create_dir_all(directory.join("temp")).unwrap();
        fs::write(home.join("agent/config.yml"),serde_json::to_vec_pretty(&json!({
            "features":{"unexpectedStopDetection":"none"},"retry":{"enabled":false,"modelFallback":false},"xdev":{"enabled":false},"edit":{"mode":"hashline"},
            "task":{"batch":batch,"structured":false,"maxConcurrency":2,"isolation":{"enabled":true,"merge":merge,"apply":apply}},
            "isolation":{"backend":"rcopy"},"async":{"enabled":asynchronous}
        })).unwrap()).unwrap();
        for (agent, marker) in [("g3-task-a", CHILD_A), ("g3-task-b", CHILD_B)] {
            fs::write(project.join(format!(".ara/agents/{agent}.md")),format!("---\nname: {agent}\ndescription: Native Task isolation fixture agent\ntools: write, bash\nspawns: '*'\n---\n{marker}\nUse the actual native write and Bash tools for the prescribed fixture work.\n")).unwrap();
        }
        git(&project, &["init", "-q", "-b", "main"]);
        git(&project, &["config", "user.name", "Native Task fixture"]);
        git(
            &project,
            &["config", "user.email", "native-task@example.invalid"],
        );
        git(&project, &["config", "core.autocrlf", "false"]);
        fs::write(project.join("user.txt"), "base user content\n").unwrap();
        git(&project, &["add", "."]);
        git(&project, &["commit", "-qm", "actual task baseline"]);
        fs::write(project.join("user.txt"), "user staged WIP\n").unwrap();
        git(&project, &["add", "user.txt"]);
        fs::write(project.join("untracked-user"), "untracked user WIP\n").unwrap();
        let initial_head = git(&project, &["rev-parse", "HEAD"]);
        let initial_index = fs::read(project.join(".git/index")).unwrap();
        Self {
            directory,
            home,
            project,
            initial_head,
            initial_index,
            keep_alive: keep,
            parent_tools: "read,write,bash,task,wait".into(),
        }
    }
    fn command(&self, url: &str, mode: &str) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_ara"));
        command.env_clear();
        for key in [
            "PATH",
            "PATHEXT",
            "SYSTEMROOT",
            "WINDIR",
            "SYSTEMDRIVE",
            "COMSPEC",
            "ARA_TEST_BUN",
            "ARA_TEST_IMAGE_HOST_ENTRY",
            "ARA_TEST_IMAGE_PROJECT_DIR",
            "ARA_TEST_TLAPLUS_GRAMMAR",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("APPDATA", self.home.join("AppData/Roaming"))
            .env("LOCALAPPDATA", self.home.join("AppData/Local"))
            .env("TEMP", self.directory.join("temp"))
            .env("TMP", self.directory.join("temp"))
            .env("TMPDIR", self.directory.join("temp"))
            .env("ARA_HOME", &self.home)
            .env("ARA_API_KEY", "sk-native-task-loopback-fixture-only")
            .env(
                "ARA_TEST_TASK_KEEP_ALIVE",
                if self.keep_alive { "true" } else { "false" },
            )
            .env("ARA_TEST_CHILD_ARTIFACT_DIR", &self.directory)
            .args([
                "--mode",
                mode,
                "--api",
                "openai-completions",
                "--model",
                "fake-model",
                "--base-url",
                url,
                "--no-skills",
                "--tools",
                &self.parent_tools,
                "--compact-threshold",
                "0",
                "--cwd",
            ])
            .arg(&self.project)
            .arg("--session-dir")
            .arg(self.home.join("sessions"))
            .kill_on_drop(false);
        command
    }
    fn preserved_wip(&self) {
        assert_eq!(
            fs::read_to_string(self.project.join("user.txt")).unwrap(),
            "user staged WIP\n"
        );
        assert_eq!(
            fs::read_to_string(self.project.join("untracked-user")).unwrap(),
            "untracked user WIP\n"
        );
        assert!(git(&self.project, &["diff", "--cached"]).contains("user staged WIP"));
    }
}
fn git(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "private Task Git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .trim_end()
        .to_owned()
}
fn lines(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}
fn result(receipts: &[Value], index: usize) -> &Value {
    assert_eq!(receipts[index]["success"], true, "{}", receipts[index]);
    &receipts[index]["data"]["result"]
}
fn terminal(events: &[Value], id: &str) -> Value {
    let found: Vec<_> = events
        .iter()
        .filter(|row| row["type"] == "tool_execution_end" && row["toolCallId"] == id)
        .collect();
    assert_eq!(found.len(), 1, "actual Native result {id}");
    assert_ne!(found[0]["isError"], true, "{}", found[0]);
    found[0].clone()
}
async fn wait_exit(bank: &Bank, child: &mut tokio::process::Child) -> std::process::ExitStatus {
    let pid = child.id();
    fs::write(
        bank.directory.join("process.json"),
        json!({"pid":pid,"state":"running"}).to_string(),
    )
    .unwrap();
    match tokio::time::timeout(BOUND, child.wait()).await {
        Ok(status) => {
            let status = status.unwrap();
            fs::write(
                bank.directory.join("process.json"),
                json!({"pid":pid,"state":"exited","success":status.success(),"code":status.code()})
                    .to_string(),
            )
            .unwrap();
            status
        }
        Err(_) => {
            fs::write(bank.directory.join("process.json"),json!({"pid":pid,"state":"unknown","rule":"no automatic replay or kill; reconcile original process and receipts"}).to_string()).unwrap();
            panic!("ordinary Task process exceeded bound; retained PID {pid:?}")
        }
    }
}
async fn fixture(bank: &Bank, up: &RoutedUpstream, operations: Vec<Value>) -> (Vec<Value>, Value) {
    fixture_with_exit(bank, up, operations, true).await
}
async fn fixture_with_exit(
    bank: &Bank,
    up: &RoutedUpstream,
    operations: Vec<Value>,
    expect_success: bool,
) -> (Vec<Value>, Value) {
    let scenario = bank.directory.join("scenario.json");
    fs::write(&scenario, serde_json::to_vec_pretty(&operations).unwrap()).unwrap();
    let mut command = bank.command(&up.url(), "rpc");
    command.env("ARA_TEST_PEER_SCENARIO", scenario).env(
        "ARA_TEST_PEER_FRONTIERS",
        bank.directory.join("frontiers.json"),
    );
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            fs::File::create(bank.directory.join("stdout.txt")).unwrap(),
        ))
        .stderr(Stdio::from(
            fs::File::create(bank.directory.join("stderr.txt")).unwrap(),
        ));
    let mut child = command.spawn().unwrap();
    let status = wait_exit(bank, &mut child).await;
    assert_eq!(
        status.success(),
        expect_success,
        "{}",
        fs::read_to_string(bank.directory.join("stderr.txt")).unwrap()
    );
    let stdout = fs::read_to_string(bank.directory.join("stdout.txt")).unwrap();
    let receipts = serde_json::from_str(
        stdout
            .lines()
            .find_map(|line| line.strip_prefix("PEER_FIXTURE_RECEIPTS="))
            .expect("actual fixture receipts"),
    )
    .unwrap();
    let teardown: Value = serde_json::from_str(
        stdout
            .lines()
            .find_map(|line| line.strip_prefix("PEER_FIXTURE_TEARDOWN="))
            .expect("actual EOF teardown receipt"),
    )
    .unwrap();
    if expect_success {
        assert_eq!(teardown["actualEOF"]["code"], 0);
    } else {
        assert!(
            teardown["actualEOF"]["error"].is_string()
                || teardown["actualEOF"]["code"]
                    .as_i64()
                    .is_some_and(|code| code != 0),
            "actual Unknown EOF cannot become success"
        );
        assert_eq!(teardown["lifecycleRootSnapshot"]["settled"], false);
    }
    (receipts, teardown)
}

pub async fn isolated_lifecycle() {
    for merge in ["patch", "branch"] {
        let bank = Bank::new(
            &format!("g3-lifecycle-{merge}"),
            merge,
            true,
            true,
            false,
            false,
        );
        let mut child = child_script("agent.txt", "initial actual child edit\n", true);
        child.extend(child_script(
            "warm-only.txt",
            "follow-up retained until release\n",
            false,
        ));
        let up = RoutedUpstream::start(
            &bank.directory,
            vec![
                call(
                    "g3-native-task",
                    "task",
                    task_args(
                        "g3-task-a",
                        "Produce the actual isolated file and Bash receipt",
                    ),
                ),
                answer("Parent saw actual Task result."),
            ],
            child,
            vec![],
        )
        .await;
        let (receipts,_)=fixture(&bank,&up,vec![json!({"op":"root_prompt","text":"Invoke the real native Task"}),json!({"op":"task_alias","alias":"worker"}),
            json!({"op":"park","alias":"worker"}),json!({"op":"task_snapshot","alias":"worker"}),json!({"op":"task_cold_probe","alias":"worker"}),json!({"op":"revive","alias":"worker"}),
            json!({"op":"send","to":"worker","body":"Perform warm follow-up in the same actual isolated cwd","wait":true}),json!({"op":"task_snapshot","alias":"worker"}),
            json!({"op":"task_release","alias":"worker","observerDrop":true})]).await;
        let first = result(&receipts, 1);
        let parked = result(&receipts, 3);
        let cold = result(&receipts, 4);
        let warm = result(&receipts, 7);
        let released = result(&receipts, 8);
        assert_eq!(cold["rejected"], true);
        assert_eq!(cold["cwdExists"], true);
        assert!(cold["error"].as_str().unwrap().contains("history-only"));
        assert_eq!(
            cold["before"], cold["after"],
            "cold factory refusal preserves the exact parked owner"
        );
        let identity = &first["record"]["key"];
        assert_eq!(identity, &parked["record"]["key"]);
        assert_eq!(identity, &warm["record"]["key"]);
        assert_eq!(
            first["resource"]["resource_id"],
            parked["resource"]["resource_id"]
        );
        assert_eq!(first["resource"]["cwd"], warm["resource"]["cwd"]);
        assert_eq!(result(&receipts, 2)["parked"], true);
        assert_eq!(parked["record"]["runtime"], Value::Null);
        assert_ne!(
            first["record"]["runtime"]["runtime_generation"],
            warm["record"]["runtime"]["runtime_generation"]
        );
        assert_eq!(first["init"]["isolated"], true);
        assert_eq!(first["resultReceipt"]["runSucceeded"], true);
        assert_eq!(first["resultReceipt"]["taskAccepted"], false);
        assert_eq!(
            first["resultReceipt"]["capture"]["resource_id"],
            first["resource"]["resource_id"]
        );
        assert_eq!(first["resultReceipt"]["apply"]["changes_applied"], true);
        let history_patch = PathBuf::from(
            first["record"]["history"]["patchPath"]
                .as_str()
                .expect("run-end history points to a real captured patch"),
        );
        assert!(history_patch.is_file());
        assert_ne!(
            first["resultReceipt"]["taskExecutionId"],
            first["resultReceipt"]["agentId"]
        );
        if merge == "branch" {
            assert_eq!(
                first["resultReceipt"]["capture"]["branch"]["branch_name"],
                format!("omp/task/{}", identity["id"].as_str().unwrap())
            );
        }
        assert_eq!(released["releaseReceipt"]["status"], "released");
        assert_eq!(released["cwdExists"], false);
        assert!(released["releaseError"].is_null());
        assert_ne!(
            released["releaseReceipt"]["capture"]["fingerprint"],
            first["resource"]["capture"]["fingerprint"]
        );
        assert_eq!(
            fs::read_to_string(bank.project.join("agent.txt")).unwrap(),
            "initial actual child edit\n"
        );
        assert_eq!(
            fs::read_to_string(bank.project.join("bash-proof.txt"))
                .unwrap()
                .trim(),
            "native-bash-proof"
        );
        assert!(
            !bank.project.join("warm-only.txt").exists(),
            "final release cannot replay parent apply"
        );
        bank.preserved_wip();
        let final_branch = released["releaseReceipt"]["capture"]["branch"]["branch_name"]
            .as_str()
            .expect("follow-up recovery branch");
        assert_eq!(
            git(
                &bank.project,
                &["show", &format!("{final_branch}:warm-only.txt")]
            ),
            "follow-up retained until release"
        );
        let final_patch = Path::new(
            released["releaseReceipt"]["capture"]["patch_path"]
                .as_str()
                .unwrap(),
        );
        assert!(final_patch.is_file());
        assert!(
            fs::read_to_string(final_patch)
                .unwrap()
                .contains("warm-only.txt")
        );
        let transcript = Path::new(first["record"]["session_file"].as_str().unwrap());
        let canonical = ara_cli::host_peers::roster::artifact_paths(
            transcript,
            identity["id"].as_str().unwrap(),
        );
        assert!(
            fs::read_to_string(&canonical.patch)
                .unwrap()
                .contains("warm-only.txt"),
            "final follow-up capture updates the ordinary discoverable agent patch"
        );
        let events = lines(&bank.directory.join("root.events.jsonl"));
        let task = terminal(&events, "g3-native-task");
        assert_eq!(task["result"]["details"]["taskAccepted"], false);
        let agent_id = identity["id"].as_str().unwrap();
        let child_events = lines(&bank.directory.join(format!("{agent_id}.events.jsonl")));
        terminal(&child_events, "g3-child-bash");
        assert!(
            first["rawJournal"]
                .as_str()
                .unwrap()
                .contains("g3-child-write")
        );
        assert!(
            warm["rawJournal"]
                .as_str()
                .unwrap()
                .contains("Perform warm follow-up")
        );
        assert_eq!(up.counts(), [2, 4, 0]);
        fs::write(bank.directory.join("g3-lifecycle-assertions.json"),serde_json::to_vec_pretty(&json!({"entry":"ordinary binary Native task","samePeer":identity,"sameResource":first["resource"]["resource_id"],"taskResult":first["resultReceipt"],"finalRelease":released,"requests":up.counts(),"parentWipPreserved":true,"finalApplyReplayed":false})).unwrap()).unwrap();
    }
    // M8 joins the actual ChildHost Unknown close to the exact Task lease and root receipt.
    // This remains in the same G3 lifecycle bank and uses the original fault frontier.
    let bank = Bank::new(
        "g3-lifecycle-task-unknown-close",
        "patch",
        true,
        true,
        false,
        false,
    );
    let up = RoutedUpstream::start(
        &bank.directory,
        vec![
            call(
                "g3-native-task",
                "task",
                task_args(
                    "g3-task-a",
                    "Write the actual task file before unknown close",
                ),
            ),
            answer("Parent received the original completed task."),
        ],
        child_script("agent.txt", "actual task before unknown close\n", false),
        vec![],
    )
    .await;
    let (receipts,teardown)=fixture_with_exit(&bank,&up,vec![
        json!({"op":"root_prompt","text":"Run the actual isolated Task, then retain its original close evidence"}),
        json!({"op":"task_alias","alias":"worker"}),
        json!({"op":"task_unknown_close","alias":"worker"}),
    ],false).await;
    let initial = result(&receipts, 1);
    let unknown = result(&receipts, 2);
    assert_eq!(initial["resultReceipt"]["runSucceeded"], true);
    assert_eq!(initial["resultReceipt"]["apply"]["changes_applied"], true);
    assert_eq!(unknown["fault"]["observerDropped"], true);
    assert_eq!(unknown["fault"]["sameHeldChannel"], true);
    assert_eq!(unknown["fault"]["sameFinalChannel"], true);
    assert_eq!(unknown["sameChannelAfterRoot"], true);
    assert_eq!(
        unknown["fault"]["originalError"],
        unknown["fault"]["duplicateError"]
    );
    assert_eq!(unknown["rootReceipt"]["settled"], false);
    assert_eq!(unknown["releaseReceipt"]["status"], "retained");
    assert_eq!(unknown["cwdExists"], true);
    assert_eq!(
        unknown["resourceBefore"]["resource_id"],
        initial["resource"]["resource_id"]
    );
    assert_eq!(unknown["resourceAfter"]["cwd"], initial["resource"]["cwd"]);
    assert_eq!(unknown["resourceAfter"]["sealed"], true);
    assert_eq!(
        unknown["resourceAfter"]["apply_flights"],
        initial["resource"]["apply_flights"]
    );
    assert_eq!(
        unknown["releaseReceipt"]["capture"]["capture_id"],
        initial["resource"]["capture"]["capture_id"],
        "unknown writer cannot trigger a new capture or parent apply"
    );
    let error = unknown["fault"]["originalError"].as_str().unwrap();
    assert!(
        unknown["releaseReceipt"]["error"]
            .as_str()
            .unwrap()
            .contains(error)
    );
    assert!(
        unknown["rootReceipt"]["peers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["key"] == initial["record"]["key"]
                && row["settled"] == false
                && row["error"]
                    .as_str()
                    .is_some_and(|value| value.contains(error)))
    );
    let cwd = Path::new(unknown["resourceAfter"]["cwd"].as_str().unwrap());
    assert!(cwd.is_dir());
    assert!(cwd.parent().unwrap().join("owner.json").is_file());
    assert_eq!(
        fs::read_to_string(cwd.join("agent.txt")).unwrap(),
        "actual task before unknown close\n"
    );
    let durable = Path::new(
        unknown["releaseReceipt"]["durable_receipt"]
            .as_str()
            .unwrap(),
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(durable).unwrap()).unwrap(),
        unknown["releaseReceipt"]
    );
    assert!(
        !unknown["resourceAfter"]["effects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|effect| effect["phase"] == "backend_stop_and_remove")
    );
    assert_eq!(
        fs::read_to_string(bank.project.join("agent.txt")).unwrap(),
        "actual task before unknown close\n"
    );
    bank.preserved_wip();
    assert_eq!(
        up.counts(),
        [2, 2, 0],
        "Unknown close and root shutdown never replay model work"
    );
    fs::write(bank.directory.join("g3-task-unknown-close-assertions.json"),serde_json::to_vec_pretty(&json!({"initial":initial,"unknown":unknown,"teardown":teardown,"requests":up.counts(),"parentWipPreserved":true,"originalResourceRetained":cwd,"parentApplyReplayed":false})).unwrap()).unwrap();
}

pub async fn async_batch() {
    let bank = Bank::new("g3-async-batch", "patch", true, true, true, true);
    let args = json!({"context":"Both tasks independently exercise actual native isolation.","tasks":[task_args("g3-task-a","Task A writes batch-a.txt"),task_args("g3-task-b","Task B writes batch-b.txt")]});
    let up = RoutedUpstream::start_gated(
        &bank.directory,
        vec![
            call("g3-async-task", "task", args),
            call("g3-actual-wait", "wait", json!({})),
            answer("Both actual Task jobs were observed."),
            answer("The original delayed Task completion was observed."),
        ],
        child_script("batch-a.txt", "A actual task\n", false),
        child_script("batch-b.txt", "B actual task\n", false),
        true,
    )
    .await;
    let (receipts,_)=fixture(&bank,&up,vec![json!({"op":"root_prompt","text":"Launch the real batch Task and observe it with actual wait"}),
        json!({"op":"task_alias","alias":"a"}),json!({"op":"task_alias","alias":"b"}),json!({"op":"task_release","alias":"a"}),json!({"op":"task_release","alias":"b"})]).await;
    let events = lines(&bank.directory.join("root.events.jsonl"));
    let task = terminal(&events, "g3-async-task");
    let wait = terminal(&events, "g3-actual-wait");
    assert!(
        bank.directory
            .join("wait-observed-original-jobs.json")
            .is_file(),
        "actual Wait watch released original held child response bytes"
    );
    assert_eq!(
        task["result"]["details"]["running"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        task["result"]["details"]["results"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(task["result"]["details"]["taskAccepted"], false);
    assert!(!wait["result"]["content"].to_string().is_empty());
    let a = result(&receipts, 1);
    let b = result(&receipts, 2);
    assert_ne!(a["record"]["key"], b["record"]["key"]);
    assert_ne!(a["resource"]["cwd"], b["resource"]["cwd"]);
    for row in [a, b] {
        assert_eq!(row["resultReceipt"]["runSucceeded"], true);
        assert_eq!(row["resultReceipt"]["taskAccepted"], false);
        assert_eq!(row["resultReceipt"]["apply"]["changes_applied"], true);
    }
    assert_eq!(result(&receipts, 3)["releaseReceipt"]["status"], "released");
    assert_eq!(result(&receipts, 4)["releaseReceipt"]["status"], "released");
    assert_eq!(
        fs::read_to_string(bank.project.join("batch-a.txt")).unwrap(),
        "A actual task\n"
    );
    assert_eq!(
        fs::read_to_string(bank.project.join("batch-b.txt")).unwrap(),
        "B actual task\n"
    );
    bank.preserved_wip();
    // Wait consumes a completed Task; a later original completion may wake the parent once.
    let counts = up.counts();
    assert!(matches!(counts[0], 3 | 4), "bounded parent continuation, no retry loop: {counts:?}");
    assert_eq!([counts[1], counts[2]], [2, 2], "original child work is never replayed");
    let routed = lines(&bank.directory.join("routed-requests.jsonl"));
    let parent_requests: Vec<_> = routed.iter().filter(|row| row["route"] == "root").collect();
    if parent_requests.len() == 4 {
        let last = parent_requests[3]["body"]["messages"].as_array().unwrap().last().unwrap();
        assert_eq!(last["role"], "user");
        let notice = last["content"].to_string();
        assert!(notice.contains("<system-notice>"));
        assert!([a, b].iter().any(|row| notice.contains(row["record"]["key"]["id"].as_str().unwrap())));
    }
    fs::write(bank.directory.join("g3-async-assertions.json"),serde_json::to_vec_pretty(&json!({"nativeTask":task,"nativeWait":wait,"tasks":[a,b],"requestCounts":up.counts(),"parentWipPreserved":true})).unwrap()).unwrap();
}

struct WireProcess {
    child: tokio::process::Child,
    input: Option<tokio::process::ChildStdin>,
    output: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    frames: Vec<Value>,
    directory: PathBuf,
}
impl WireProcess {
    fn spawn(mut command: tokio::process::Command, directory: &Path) -> Self {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(
                fs::File::create(directory.join("stderr.txt")).unwrap(),
            ));
        let mut child = command.spawn().unwrap();
        fs::write(
            directory.join("process.json"),
            json!({"pid":child.id(),"state":"running","entry":"ordinary wire process"}).to_string(),
        )
        .unwrap();
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().unwrap()).lines();
        Self {
            child,
            input,
            output,
            frames: vec![],
            directory: directory.to_path_buf(),
        }
    }
    async fn send(&mut self, value: Value) {
        let input = self.input.as_mut().unwrap();
        input
            .write_all(format!("{value}\n").as_bytes())
            .await
            .unwrap();
        input.flush().await.unwrap();
    }
    async fn until(&mut self, predicate: impl Fn(&Value) -> bool) -> Value {
        let waited = tokio::time::timeout(Duration::from_secs(40), async {
            loop {
                let line = self
                    .output
                    .next_line()
                    .await
                    .unwrap()
                    .expect("actual binary ended before requested protocol receipt");
                let mut raw = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(self.directory.join("stdout.txt"))
                    .unwrap();
                writeln!(raw, "{line}").unwrap();
                if let Ok(frame) = serde_json::from_str::<Value>(&line) {
                    self.frames.push(frame.clone());
                    if predicate(&frame) {
                        return frame;
                    }
                }
            }
        })
        .await;
        match waited {
            Ok(frame) => frame,
            Err(_) => {
                fs::write(self.directory.join("process.json"),json!({"pid":self.child.id(),"state":"unknown","rule":"wire wait timeout; reconcile original process before replay"}).to_string()).unwrap();
                panic!("ordinary Task protocol bounded wait; preserve original process");
            }
        }
    }
    async fn rpc(&mut self, id: &str, kind: &str, fields: Value) -> Value {
        let mut command = fields;
        command["id"] = id.into();
        command["type"] = kind.into();
        self.send(command).await;
        let response = self
            .until(|frame| frame["type"] == "response" && frame["id"] == id)
            .await;
        assert_eq!(response["success"], true, "{response}");
        response["data"].clone()
    }
    async fn acp(&mut self, id: &str, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await;
        let response = self
            .until(|frame| frame["id"] == id && frame.get("method").is_none())
            .await;
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }
    async fn finish(self, bank: &Bank) -> Vec<Value> {
        let Self {
            mut child,
            input,
            mut output,
            mut frames,
            directory,
        } = self;
        drop(input);
        let drain = async {
            while let Some(line) = output.next_line().await.unwrap() {
                let mut raw = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(directory.join("stdout.txt"))
                    .unwrap();
                writeln!(raw, "{line}").unwrap();
                if let Ok(frame) = serde_json::from_str::<Value>(&line) {
                    frames.push(frame);
                }
            }
        };
        let (status, ()) = tokio::join!(wait_exit(bank, &mut child), drain);
        assert!(
            status.success(),
            "{}",
            fs::read_to_string(bank.directory.join("stderr.txt")).unwrap()
        );
        fs::write(
            bank.directory.join("wire-frames.json"),
            serde_json::to_vec_pretty(&frames).unwrap(),
        )
        .unwrap();
        frames
    }
}
fn journal_paths(root: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                journal_paths(&entry.path(), out);
            } else if entry.path().extension().is_some_and(|ext| ext == "jsonl") {
                out.push(entry.path());
            }
        }
    }
}
fn actual_journals(bank: &Bank) -> Vec<Value> {
    let mut paths = Vec::new();
    journal_paths(&bank.home.join("sessions"), &mut paths);
    let mut journals = Vec::new();
    for path in paths {
        let Ok(journal) = ara_session::SessionJournal::open_with_blob_directory(
            &path,
            &bank.home.join("agent/blobs"),
        ) else {
            continue;
        };
        let rows = journal
            .entries()
            .iter()
            .filter_map(|entry| entry.observed_message_body().ok().flatten())
            .map(|body| ara_session::native_checked_serde(&body).unwrap())
            .collect::<Vec<_>>();
        journals.push(json!({"path":path,"id":journal.session_id(),"header":journal.header(),"init":journal.latest_child_init().unwrap(),"rows":rows}));
    }
    journals
}

pub async fn ordinary_entries() {
    for entry in ["print", "line", "rpc", "acp"] {
        let bank = Bank::new(
            &format!("g3-entry-{entry}"),
            "patch",
            true,
            false,
            entry == "print",
            false,
        );
        if entry == "print" {
            // Global async is enabled; this actual agent frontmatter selects the blocking path.
            let definition = bank.project.join(".ara/agents/g3-task-a.md");
            let original = fs::read_to_string(&definition).unwrap();
            fs::write(
                definition,
                original.replace("tools: write, bash", "tools: write, bash\nblocking: true"),
            )
            .unwrap();
        }
        let up = RoutedUpstream::start(
            &bank.directory,
            vec![
                call(
                    "g3-entry-task",
                    "task",
                    task_args(
                        "g3-task-a",
                        "Perform actual Task entry file and Bash effects",
                    ),
                ),
                answer("Original parent Task entry finished."),
            ],
            child_script("entry-proof.txt", "actual entry proof\n", true),
            vec![],
        )
        .await;
        let frames = match entry {
            "print" | "line" => {
                let mut command = bank.command(&up.url(), "json");
                if entry == "print" {
                    command
                        .arg("--print")
                        .arg("Run actual Native Task from print entry")
                        .stdin(Stdio::null());
                } else {
                    command.arg("--repl").stdin(Stdio::piped());
                }
                command
                    .stdout(Stdio::from(
                        fs::File::create(bank.directory.join("stdout.txt")).unwrap(),
                    ))
                    .stderr(Stdio::from(
                        fs::File::create(bank.directory.join("stderr.txt")).unwrap(),
                    ));
                let mut child = command.spawn().unwrap();
                if entry == "line" {
                    let mut input = child.stdin.take().unwrap();
                    input
                        .write_all(b"Run actual Native Task from line REPL entry\n")
                        .await
                        .unwrap();
                    input.shutdown().await.unwrap();
                }
                let status = wait_exit(&bank, &mut child).await;
                assert!(
                    status.success(),
                    "{entry}: {}",
                    fs::read_to_string(bank.directory.join("stderr.txt")).unwrap()
                );
                lines(&bank.directory.join("stdout.txt"))
            }
            "rpc" => {
                let mut process =
                    WireProcess::spawn(bank.command(&up.url(), "rpc"), &bank.directory);
                process.until(|frame| frame["type"] == "ready").await;
                let before = process.rpc("before", "get_state", json!({})).await;
                process
                    .rpc(
                        "entry-prompt",
                        "prompt",
                        json!({"message":"Run actual Native Task from RPC entry"}),
                    )
                    .await;
                process.until(|frame| frame["type"] == "agent_end").await;
                let after = process.rpc("after", "get_state", json!({})).await;
                assert_eq!(before["sessionId"], after["sessionId"]);
                assert_eq!(before["sessionFile"], after["sessionFile"]);
                process.finish(&bank).await
            }
            "acp" => {
                let mut process =
                    WireProcess::spawn(bank.command(&up.url(), "acp"), &bank.directory);
                let initialized=process.acp("initialize","initialize",json!({"protocolVersion":1,"clientInfo":{"name":"native-task-proof","version":"1"},"clientCapabilities":{}})).await;
                assert_eq!(initialized["protocolVersion"], 1);
                let opened = process
                    .acp(
                        "new",
                        "session/new",
                        json!({"cwd":bank.project,"mcpServers":[]}),
                    )
                    .await;
                let session = opened["sessionId"].as_str().unwrap();
                let completed=process.acp("entry-prompt","session/prompt",json!({"sessionId":session,"prompt":[{"type":"text","text":"Run actual Native Task from ACP entry"}]})).await;
                assert_eq!(completed["stopReason"], "end_turn");
                process.finish(&bank).await
            }
            _ => unreachable!(),
        };
        assert_eq!(
            fs::read_to_string(bank.project.join("entry-proof.txt")).unwrap(),
            "actual entry proof\n"
        );
        assert_eq!(
            fs::read_to_string(bank.project.join("bash-proof.txt"))
                .unwrap()
                .trim(),
            "native-bash-proof"
        );
        bank.preserved_wip();
        let journals = actual_journals(&bank);
        let roots: Vec<_> = journals
            .iter()
            .filter(|j| j["rows"].to_string().contains("g3-entry-task"))
            .collect();
        assert_eq!(
            roots.len(),
            1,
            "one original parent Session journal for {entry}"
        );
        let children: Vec<_> = journals
            .iter()
            .filter(|j| j["init"]["isolated"] == true && j["init"]["agent"] == "g3-task-a")
            .collect();
        assert_eq!(
            children.len(),
            1,
            "actual isolated child Session for {entry}"
        );
        assert_ne!(roots[0]["id"], children[0]["id"]);
        assert!(children[0]["rows"].to_string().contains("g3-child-write"));
        assert!(children[0]["rows"].to_string().contains("g3-child-bash"));
        let task_results: Vec<_> = roots[0]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["role"] == "toolResult" && row["toolCallId"] == "g3-entry-task")
            .collect();
        assert_eq!(task_results.len(), 1);
        assert_ne!(task_results[0]["isError"], true);
        assert_eq!(task_results[0]["details"]["taskAccepted"], false);
        let result = &task_results[0]["details"]["results"][0];
        assert_eq!(result["runSucceeded"], true);
        assert_eq!(result["apply"]["changes_applied"], true);
        assert_eq!(result["agentId"], children[0]["id"]);
        let captured = PathBuf::from(result["capture"]["patch_path"].as_str().unwrap());
        assert!(captured.is_file());
        let cwd = PathBuf::from(children[0]["header"]["cwd"].as_str().unwrap());
        assert!(
            !cwd.exists(),
            "one-shot real resource cleanup at {entry} EOF"
        );
        assert_eq!(up.counts(), [2, 2, 0]);
        fs::write(bank.directory.join("g3-entry-assertions.json"),serde_json::to_vec_pretty(&json!({"entry":entry,"frames":frames,"journals":journals,"requestCounts":up.counts(),"parentWipPreserved":true,"actualChildCwdRemoved":cwd,"initialHead":bank.initial_head,"initialIndexBytes":bank.initial_index.len()})).unwrap()).unwrap();
    }

    // Exercise the real bundled definitions through the ordinary binary Native Task.
    // Their declared find/lsp entries are desired tools, not new parent authority.
    for agent in ["scout", "reviewer", "security-reviewer"] {
        let mut bank = Bank::new(
            &format!("g3-entry-bundled-{agent}"), "patch", false, false, false, false,
        );
        bank.parent_tools = "read,grep,glob,task,wait".into();
        let config_path = bank.home.join("agent/config.yml");
        let mut config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        config["task"]["structured"] = json!(true);
        fs::write(config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
        let (payload, model_role, declared, required) = match agent {
            "scout" => (
                json!({"summary":"Read the actual staged fixture content.","files":[{"path":"user.txt","description":"Existing staged user WIP"}],"architecture":"A single fixture file; no edits performed."}),
                json!("smol"),
                json!(["read", "find", "grep", "glob", "web_search"]),
                json!(["summary", "files", "architecture"]),
            ),
            "reviewer" => (
                json!({"overall_correctness":"correct","explanation":"The assigned fixture file was read; no defect is asserted.","confidence":0.9}),
                json!("slow"),
                json!(["read", "find", "grep", "glob", "bash", "lsp", "web_search", "ast_grep"]),
                json!(["overall_correctness", "explanation", "confidence"]),
            ),
            "security-reviewer" => (
                json!({"coverage_summary":"Read user.txt only; no vulnerability is asserted."}),
                Value::Null,
                json!(["read", "find", "grep", "glob", "lsp", "ast_grep"]),
                json!(["coverage_summary"]),
            ),
            _ => unreachable!(),
        };
        let up = RoutedUpstream::start(
            &bank.directory,
            vec![
                call("g3-bundled-task", "task", json!({"agent":agent,"task":"Read user.txt and return the original agent output contract.","solutionSpace":"Inspect only user.txt through granted read-only tools; return the bundled agent's original structured output without modifying files.","isolated":true})),
                answer("The original bundled specialist completed its bounded fixture scope."),
            ],
            vec![
                call("g3-bundled-read", "read", json!({"path":"user.txt"})),
                // Deliberately violate the requested read-only task. A coordination
                // device write, if present, must still deny this local file effect.
                call("g3-bundled-denied-write", "write", json!({"path":"bundled-forbidden.txt","content":"must never be written"})),
                call("g3-bundled-yield", "yield", json!({"data":payload})),
            ],
            vec![],
        ).await;
        let mut process = WireProcess::spawn(bank.command(&up.url(), "rpc"), &bank.directory);
        process.until(|frame| frame["type"] == "ready").await;
        process.rpc("bundled-prompt", "prompt", json!({"message":"Invoke the actual bundled specialist through Native Task."})).await;
        process.until(|frame| frame["type"] == "agent_end").await;
        let frames = process.finish(&bank).await;
        let journals = actual_journals(&bank);
        let children: Vec<_> = journals.iter().filter(|row| row["init"]["agent"] == agent).collect();
        assert_eq!(children.len(), 1, "one real bundled {agent} ChildHost journal");
        let child = children[0];
        let init = &child["init"];
        assert_eq!(init["readOnly"], true);
        assert_eq!(init["modelRole"], model_role);
        assert_eq!(init["araTaskAgentSource"], format!("embedded:{agent}.md"));
        assert_eq!(init["araTaskDeclaredTools"], declared);
        assert_eq!(init["restrictToolNames"], false);
        assert_eq!(init["isolated"], true);
        let actual: Vec<_> = init["tools"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        for name in ["read", "grep", "glob", "yield"] {
            assert!(actual.contains(&name), "actual {agent} child retains {name}");
        }
        for name in ["find", "lsp", "write", "bash", "web_search", "ast_grep"] {
            assert!(!actual.contains(&name), "{agent} must not acquire parent-ungranted {name}");
        }
        let schema = &init["outputSchema"];
        assert_eq!(schema["type"], "object");
        let schema_required = schema["required"].as_array().unwrap();
        assert_eq!(schema_required.len(), required.as_array().unwrap().len());
        for name in required.as_array().unwrap() {
            assert!(schema_required.contains(name), "original bundled required field {name}");
        }
        match agent {
            "scout" => {
                assert_eq!(init["readSummarize"], false);
                assert_eq!(schema["properties"]["files"]["items"]["properties"]["path"]["type"], "string");
                assert_eq!(schema["properties"]["report"]["type"], "string");
            }
            "reviewer" => {
                assert_eq!(init["spawns"], "scout");
                assert_eq!(schema["properties"]["overall_correctness"]["enum"], json!(["correct", "incorrect"]));
                assert_eq!(schema["properties"]["confidence"]["type"], "number");
                assert_eq!(schema["properties"]["findings"]["type"], "array");
            }
            "security-reviewer" => {
                assert_eq!(schema["properties"]["coverage_summary"]["type"], "string");
                for name in ["findings", "reviewed_paths", "deferred"] {
                    assert_eq!(schema["properties"][name]["type"], "array");
                }
            }
            _ => unreachable!(),
        }
        let rows = child["rows"].as_array().unwrap();
        let read: Vec<_> = rows.iter().filter(|row| row["role"] == "toolResult" && row["toolCallId"] == "g3-bundled-read").collect();
        assert_eq!(read.len(), 1);
        assert_ne!(read[0]["isError"], true);
        assert!(read[0].to_string().contains("user staged WIP"));
        let denied: Vec<_> = rows.iter().filter(|row| row["role"] == "toolResult" && row["toolCallId"] == "g3-bundled-denied-write").collect();
        assert_eq!(denied.len(), 1);
        assert_eq!(denied[0]["isError"], true, "local write denied by the actual tool pipeline");
        let yielded: Vec<_> = rows.iter().filter(|row| row["role"] == "toolResult" && row["toolCallId"] == "g3-bundled-yield").collect();
        assert_eq!(yielded.len(), 1);
        assert_ne!(yielded[0]["isError"], true);
        let roots: Vec<_> = journals.iter().filter(|row| row["rows"].as_array().unwrap().iter().any(|message| message["role"] == "toolResult" && message["toolCallId"] == "g3-bundled-task")).collect();
        assert_eq!(roots.len(), 1);
        assert_ne!(roots[0]["id"], child["id"]);
        let task = roots[0]["rows"].as_array().unwrap().iter().find(|row| row["role"] == "toolResult" && row["toolCallId"] == "g3-bundled-task").unwrap();
        assert_ne!(task["isError"], true);
        let completed = &task["details"]["results"][0];
        assert_eq!(completed["agent"], agent);
        assert_eq!(completed["agentId"], child["id"]);
        assert_eq!(completed["agentSource"], init["araTaskAgentSource"]);
        assert_eq!(completed["runSucceeded"], true);
        assert_eq!(completed["acceptedYield"], true);
        assert_eq!(completed["taskAccepted"], false);
        assert_eq!(completed["schemaOverridden"], false);
        assert_eq!(completed["data"], payload);
        let patch = PathBuf::from(completed["capture"]["patch_path"].as_str().unwrap());
        assert!(!fs::read_to_string(patch).unwrap().contains("bundled-forbidden.txt"));
        assert!(!bank.project.join("bundled-forbidden.txt").exists());
        let requests = lines(&bank.directory.join("routed-requests.jsonl"));
        let child_requests: Vec<_> = requests.iter().filter(|row| row["route"] == "child-a").collect();
        assert_eq!(child_requests.len(), 3);
        let yield_schema = ara_cli::child_contract::schema::build(Some(schema));
        assert!(yield_schema.error.is_none());
        for request in &child_requests {
            let tools = request["body"]["tools"].as_array().unwrap();
            let names: Vec<_> = tools.iter().filter_map(|tool| tool["function"]["name"].as_str()).collect();
            for name in ["read", "grep", "glob", "yield"] { assert!(names.contains(&name)); }
            for name in ["find", "lsp", "bash", "web_search", "ast_grep"] { assert!(!names.contains(&name)); }
            let wire_yield = tools.iter().find(|tool| tool["function"]["name"] == "yield").unwrap();
            let wire_schema = &wire_yield["function"]["parameters"];
            assert_eq!(wire_schema["type"], "object");
            for name in ["type", "data", "error"] {
                assert!(wire_schema["properties"].get(name).is_some());
            }
            // Native Completions makes optional parameters nullable in strict
            // mode. Inspect the schema-bearing data branch after that actual
            // adaptation, preserving the original bundled required fields.
            let wire_data = if wire_yield["function"]["strict"] == true {
                let all_required = wire_schema["required"].as_array().unwrap();
                for name in ["type", "data", "error"] {
                    assert!(all_required.contains(&json!(name)));
                }
                wire_schema["properties"]["data"]["anyOf"].as_array().unwrap()
                    .iter().find(|branch| branch["type"] == "object").unwrap()
            } else {
                &wire_schema["properties"]["data"]
            };
            assert_eq!(wire_data["type"], "object");
            assert_eq!(wire_data["properties"].as_object().unwrap().keys().collect::<Vec<_>>(),
                schema["properties"].as_object().unwrap().keys().collect::<Vec<_>>());
            for name in required.as_array().unwrap() {
                assert!(wire_data["required"].as_array().unwrap().contains(name));
            }
        }
        let parent_request = requests.iter().find(|row| row["route"] == "root").unwrap();
        let parent_names: Vec<_> = parent_request["body"]["tools"].as_array().unwrap().iter().filter_map(|tool| tool["function"]["name"].as_str()).collect();
        for name in ["read", "grep", "glob", "task", "wait"] { assert!(parent_names.contains(&name)); }
        for name in ["find", "lsp", "bash"] { assert!(!parent_names.contains(&name)); }
        assert_eq!(up.counts(), [2, 3, 0]);
        bank.preserved_wip();
        fs::write(bank.directory.join("g3-bundled-entry-assertions.json"), serde_json::to_vec_pretty(&json!({"agent":agent,"frames":frames,"journals":journals,"requests":requests,"requestCounts":up.counts(),"parentWipPreserved":true,"localWriteDenied":true})).unwrap()).unwrap();
    }

    // Explicit caller tools remain an authority request. A later invalid item
    // rejects the whole batch before even the first valid child is created.
    for forbidden in ["find", "lsp", "write"] {
        let mut bank = Bank::new(&format!("g3-entry-caller-denied-{forbidden}"), "patch", false, false, false, true);
        bank.parent_tools = "read,grep,glob,task,wait".into();
        let up = RoutedUpstream::start(&bank.directory, vec![
            call("g3-caller-denied-task", "task", json!({"context":"Validate the complete batch's authority before creating any child or isolated workspace; parent grants only read, grep, glob, task and wait.","tasks":[
                {"agent":"scout","task":"This valid first child must not be created.","solutionSpace":"Read user.txt only if the entire batch passes preflight; do not modify files.","isolated":true},
                {"agent":"reviewer","task":"Reject this explicit unavailable authority.","solutionSpace":"Request the explicitly listed tools; refuse unavailable parent authority before any child creation or effects.","tools":["read",forbidden],"isolated":true}
            ]})),
            answer("The explicit unavailable caller tools were rejected before child creation."),
        ], vec![], vec![]).await;
        let mut process = WireProcess::spawn(bank.command(&up.url(), "rpc"), &bank.directory);
        process.until(|frame| frame["type"] == "ready").await;
        process.rpc("denied-prompt", "prompt", json!({"message":"Exercise the actual Native Task caller authority rejection."})).await;
        process.until(|frame| frame["type"] == "agent_end").await;
        let frames = process.finish(&bank).await;
        let journals = actual_journals(&bank);
        assert_eq!(journals.len(), 1, "preflight cannot create a child journal");
        assert!(journals[0]["init"].is_null());
        let denied: Vec<_> = journals[0]["rows"].as_array().unwrap().iter().filter(|row| row["role"] == "toolResult" && row["toolCallId"] == "g3-caller-denied-task").collect();
        assert_eq!(denied.len(), 1);
        assert_eq!(denied[0]["isError"], true);
        assert!(denied[0].to_string().contains("not exposed by the actual parent"));
        assert!(denied[0].to_string().contains(forbidden));
        assert_eq!(up.counts(), [2, 0, 0], "preflight rejection causes no child HTTP");
        assert!(!bank.home.join("task-worktrees").exists(), "preflight rejection causes no isolation effect");
        let root_transcript = PathBuf::from(journals[0]["path"].as_str().unwrap());
        let artifacts = root_transcript.with_extension("");
        if artifacts.is_dir() {
            assert!(!fs::read_dir(artifacts).unwrap().any(|entry| entry.unwrap().file_name().to_string_lossy().starts_with("task-")), "no Task execution artifact directory before successful preflight");
        }
        bank.preserved_wip();
        fs::write(bank.directory.join("g3-caller-denied-assertions.json"), serde_json::to_vec_pretty(&json!({"callerTool":forbidden,"frames":frames,"journals":journals,"requestCounts":up.counts(),"noChildJournal":true,"noIsolationEffect":true,"parentWipPreserved":true})).unwrap()).unwrap();
    }
}
