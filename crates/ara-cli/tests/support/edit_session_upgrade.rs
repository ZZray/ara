//! G10 grouped real-process Session Store boundary bank.
//! Included only from existing rpc_host.rs; reuse its bounded actual child,
//! controlled HTTP and journal helpers instead of another process harness.
use super::*;

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn edit_store_failed_switch_same_session_new_and_switch_back_actual_process_bank() {
    let env=Env::new();let original="def add(a, b):\n    return a - b  # BUG\n";
    std::fs::create_dir_all(env.work.path().join("src")).unwrap();std::fs::write(env.work.path().join("src/math.py"),original).unwrap();
    let edit=json!({"input":"[math.py#450E]\nPUT 2.=2:\n+    return a + b\n"}).to_string();
    let up=upstream(json!({"responses":[
        {"events":[tool_call(0,"session-read","read","{\"path\":\"src/math.py\"}"),finish("tool_calls"),done()]},
        {"events":[text("Snapshot issued."),finish("stop"),done()]},
        {"events":[tool_call(0,"after-failed-switch","edit",&edit),finish("tool_calls"),done()]},
        {"events":[text("Failed switch retained original Session."),finish("stop"),done()]},
        {"events":[tool_call(0,"after-new-session","edit",&edit),finish("tool_calls"),done()]},
        {"events":[text("New Session rejects stale snapshot."),finish("stop"),done()]},
        {"events":[tool_call(0,"after-switch-back","edit",&edit),finish("tool_calls"),done()]},
        {"events":[text("Switched Session issues no old snapshot."),finish("stop"),done()]}
    ]})).await;
    let mut command=env.command(&up.base_url(),&["--tools","read,edit","--edit-mode","hashline"]);
    command.env_remove("ARA_EDIT_VARIANT").env_remove("ARA_STRICT_EDIT_MODE");
    let mut child=RpcChild::spawn(command);child.ready();
    let prompt=|child:&mut RpcChild,id:&str| {
        child.send(json!({"id":id,"type":"prompt","message":id}));child.success(id);
        child.until(|frame|frame["type"]=="agent_end",WAIT);
    };
    let receipt=|messages:Vec<Value>,id:&str|messages.into_iter().find(|message|message["role"]=="toolResult"&&message["toolCallId"]==id).expect("actual tool receipt");
    prompt(&mut child,"read-and-issue");
    let old=child.state("old-identity");let old_file=PathBuf::from(old["sessionFile"].as_str().unwrap());
    assert!(!old["sessionId"].as_str().unwrap().is_empty());
    child.adopt("same-session-reload",json!({"id":"same-session-reload","type":"switch_session","sessionPath":old_file}));
    assert_eq!(child.state("same-identity")["sessionId"],old["sessionId"]);
    child.send(json!({"id":"failed-current-switch","type":"switch_session","sessionPath":env.home.path().join("does-not-exist.jsonl")}));
    assert_eq!(child.response("failed-current-switch")["success"],false);
    let kept=child.state("after-failure-identity");assert_eq!(kept["sessionId"],old["sessionId"]);assert_eq!(kept["sessionFile"],old["sessionFile"]);
    prompt(&mut child,"edit-retained-after-failure");
    assert_eq!(receipt(child.messages("retained-receipt"),"after-failed-switch")["isError"],false);
    assert_eq!(std::fs::read_to_string(env.work.path().join("src/math.py")).unwrap(),"def add(a, b):\n    return a + b\n");
    std::fs::write(env.work.path().join("src/math.py"),original).unwrap();
    child.adopt("new-store-boundary",json!({"id":"new-store-boundary","type":"new_session"}));
    let fresh=child.state("new-identity");assert_ne!(fresh["sessionId"],old["sessionId"]);
    prompt(&mut child,"edit-rejected-after-new");
    assert_eq!(receipt(child.messages("new-receipt"),"after-new-session")["isError"],true);
    assert_eq!(std::fs::read_to_string(env.work.path().join("src/math.py")).unwrap(),original);
    child.adopt("switch-old-store",json!({"id":"switch-old-store","type":"switch_session","sessionPath":old_file}));
    assert_eq!(child.state("restored-identity")["sessionId"],old["sessionId"]);
    prompt(&mut child,"edit-rejected-after-switch");
    assert_eq!(receipt(child.messages("switch-receipt"),"after-switch-back")["isError"],true);
    assert_eq!(std::fs::read_to_string(env.work.path().join("src/math.py")).unwrap(),original);
    child.finish(0);assert_eq!(up.served(),8);
    let entries=journal(&old_file);
    assert!(entries.iter().any(|entry|entry["message"]["toolCallId"]=="after-failed-switch"&&entry["message"]["isError"]==false));
    assert!(entries.iter().any(|entry|entry["message"]["toolCallId"]=="after-switch-back"&&entry["message"]["isError"]==true));
}
