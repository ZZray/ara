//! Coarse real CLI/RPC child family. Uses the owning original Native Session,
//! controlled provider stream, real preview/write bytes and restart journal.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_ast_edit_proposal_apply_discard_soft_reminder_and_restart_group() {
    let env = Env::new();
    std::fs::create_dir_all(env.home.path().join("agent")).unwrap();
    let file = env.work.path().join("calls.ts");
    std::fs::write(&file, "foo(1);\nfoo(2);\n").unwrap();
    let preview = json!({"ops":[{"pat":"foo($A)","out":"bar($A)"}],"paths":["calls.ts"]}).to_string();
    let discard = json!({"ops":[{"pat":"bar($A)","out":"discarded($A)"}],"paths":["calls.ts"]}).to_string();
    let accept = json!({"path":"xd://resolve","content":"approve both structural call replacements"}).to_string();
    let reject = json!({"path":"xd://reject","content":"keep the accepted call sites"}).to_string();
    let up = upstream(json!({"responses":[
        {"events":[tool_call(0,"ast-propose","ast_edit",&preview),finish("tool_calls"),done()]},
        {"events":[{"sleep_ms":500},tool_call(0,"ast-accept","write",&accept),finish("tool_calls"),done()]},
        {"events":[text("Both proposed call sites were applied."),finish("stop"),done()]},
        {"events":[tool_call(0,"ast-discard-preview","ast_edit",&discard),finish("tool_calls"),done()]},
        {"events":[{"sleep_ms":500},tool_call(0,"ast-discard","write",&reject),finish("tool_calls"),done()]},
        {"events":[text("The second proposal was discarded."),finish("stop"),done()]}
    ]}))
    .await;
    let mut child = RpcChild::spawn(
        env.selection_command(&up.base_url(), &["--api", "openai-completions", "--tools", "ast_edit,write"]),
    );
    child.ready();
    let initial = child.state("ast-original-session");
    for (id, call_id, before, after) in [
        ("ast-apply-run", "ast-propose", "foo(1);\nfoo(2);\n", "bar(1);\nbar(2);\n"),
        ("ast-reject-run", "ast-discard-preview", "bar(1);\nbar(2);\n", "bar(1);\nbar(2);\n"),
    ] {
        child.send(json!({"id":id,"type":"prompt","message":"complete the staged structural rewrite decision"}));
        child.success(id);
        let proposed =
            child.until(|frame| frame["type"] == "tool_execution_end" && frame["toolCallId"] == call_id, WAIT);
        assert_eq!(proposed["isError"], false, "{proposed}");
        assert_eq!(proposed["result"]["details"]["applied"], false);
        assert_eq!(proposed["result"]["details"]["totalReplacements"], 2);
        assert!(contains_text(&proposed["result"], ara_tools::resolve::PREVIEW_PENDING_NOTICE));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), before, "proposals must not write before resolution");
        child.until(|frame| frame["type"] == "agent_end", WAIT);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), after);
        assert_eq!(child.state(&format!("{id}-same-session"))["sessionId"], initial["sessionId"]);
    }
    let accepted = child
        .seen
        .iter()
        .find(|frame| frame["type"] == "tool_execution_end" && frame["toolCallId"] == "ast-accept")
        .unwrap();
    assert_eq!(accepted["result"]["details"]["xdev"]["inner"]["sourceResultDetails"]["applied"], true);
    assert_eq!(accepted["result"]["details"]["xdev"]["inner"]["sourceToolName"], "ast_edit");
    let discarded = child
        .seen
        .iter()
        .find(|frame| frame["type"] == "tool_execution_end" && frame["toolCallId"] == "ast-discard")
        .unwrap();
    assert_eq!(discarded["result"]["details"]["xdev"]["inner"]["action"], "discard");
    let messages = child.messages("ast-retained-history");
    assert_eq!(messages.iter().filter(|message| message["role"] == "toolResult").count(), 4);
    let final_state = child.state("ast-final-state");
    let session = PathBuf::from(final_state["sessionFile"].as_str().unwrap());
    child.finish(0);
    let records = journal(&session);
    let reminder = records
        .iter()
        .find(|entry| entry["type"] == "custom_message" && entry["customType"] == "resolve-reminder")
        .expect("resolve reminder uses the fixed OMP custom-message journal envelope");
    assert_eq!(reminder["display"], false);
    assert_eq!(reminder["details"]["toolName"], "ast_edit");
    assert_eq!(
        reminder["content"],
        concat!(
            "<system-reminder>\n",
            "`ast_edit` result above: PREVIEW — no files changed. ",
            "Finalize now with `write`: write a one-sentence plain-text reason to `xd://resolve` to APPLY, ",
            "or `xd://reject` to DISCARD.\n",
            "</system-reminder>",
        ),
    );
    assert!(records.iter().any(|entry| entry["message"]["toolCallId"] == "ast-accept"));
    assert_eq!(up.served(), 6);
    let requests = up.requests.lock().await;
    for index in [1, 4] {
        let body = &requests[index]["body"];
        assert!(
            body["tool_choice"].is_null() || body["tool_choice"] == "auto",
            "compliance must not force write before the model declines: {body}"
        );
        assert!(body["messages"].to_string().contains("PREVIEW — no files changed"));
    }
    drop(requests);
    let mut reopened = RpcChild::spawn(env.selection_command(
        &up.base_url(),
        &["--api", "openai-completions", "--tools", "ast_edit,write", "--resume", session.to_str().unwrap()],
    ));
    reopened.ready();
    assert_eq!(reopened.state("ast-reopened-state")["sessionId"], initial["sessionId"]);
    assert_eq!(reopened.messages("ast-reopened-history"), messages);
    reopened.finish(0);
    assert_eq!(up.served(), 6, "restart must not replay resolved AST edits");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "bar(1);\nbar(2);\n");
}
