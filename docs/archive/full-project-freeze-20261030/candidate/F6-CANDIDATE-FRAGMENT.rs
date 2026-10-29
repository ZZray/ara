// Append this fragment to the existing task_host_cases module; call it from the one merged gap bank.
async fn gap_native_grants() {
    for source in ["rpc", "mcp"] {
        for mode in ["allow", "deny", "catalog-absent"] {
            let bank = Bank::new(&format!("g3-gap-grant-{source}-{mode}"), "patch", false, false, false, true);
            let actual_name = if source == "rpc" { "f6_host_echo" } else { "mcp__fixture__echo" };
            let requested_name = if mode == "catalog-absent" { "f6_unregistered_capability" } else { actual_name };
            fs::write(
                bank.project.join(".ara/agents/g3-task-a.md"),
                format!("---\nname: g3-task-a\ndescription: Actual external source grant fixture\ntools: {actual_name}\nspawns: '*'\n---\n{CHILD_A}\nInvoke only the prescribed actual external capability.\n"),
            ).unwrap();
            let config_path = bank.directory.join("frontiers.json");
            let actions = if mode == "catalog-absent" { json!([]) } else {
                json!([{"source":source,"tool":actual_name,"enabled":mode == "allow"}])
            };
            fs::write(&config_path, serde_json::to_vec_pretty(&json!({
                "gates":[], "taskSourcePermissions":actions
            })).unwrap()).unwrap();
            let mut selected = task_args("g3-task-a", "Invoke the actual external source capability once.");
            let args = if mode == "allow" {
                selected
            } else {
                selected["tools"] = json!([requested_name]);
                json!({"tasks":[
                    task_args("g3-task-b", "This valid first item must not start before full-batch admission."),
                    selected
                ]})
            };
            let marker = bank.directory.join("actual-mcp-dispatched.txt");
            let child_args = if source == "rpc" {
                json!({"value":"f6 actual RPC child value"})
            } else {
                json!({"text":"f6 actual MCP child value","marker":marker})
            };
            let up = RoutedUpstream::start(
                &bank.directory,
                vec![
                    call("g3-f6-task", "task", args),
                    answer("The original Task source-grant outcome was observed."),
                ],
                if mode == "allow" {
                    vec![
                        call("g3-f6-external-call", actual_name, child_args.clone()),
                        answer("Actual external capability completed."),
                    ]
                } else { vec![] },
                vec![],
            ).await;
            let mcp_record = bank.directory.join("actual-mcp-server.json");
            let mcp_config = bank.directory.join("actual-mcp-config.json");
            let mut command = bank.command(&up.url(), "rpc");
            command.env("ARA_TEST_PEER_FRONTIERS", &config_path);
            if source == "mcp" {
                let helper = Path::new(env!("CARGO_BIN_EXE_ara-mcp-fixture"));
                assert!(helper.is_file(), "Root must prepare the existing ara-mcp-fixture binary in the same module build");
                fs::write(&mcp_config, serde_json::to_vec_pretty(&json!({
                    "name":"fixture","command":helper,"args":["--record",mcp_record]
                })).unwrap()).unwrap();
                command.arg("--mcp-config").arg(&mcp_config).args(["--mcp-allow","fixture:echo"]);
            }
            let mut process = WireProcess::spawn(command, &bank.directory);
            process.until(|frame| frame["type"] == "ready").await;
            if source == "rpc" {
                process.rpc("f6-register", "set_host_tools", json!({"tools":[{
                    "name":actual_name,"description":"Private actual source-owned RPC echo",
                    "parameters":{"type":"object","properties":{"value":{"type":"string"}},"required":["value"]}
                }]})).await;
            }
            let before = GapParentSnapshot::capture(&bank, "before-source-grant-task");
            process.rpc("f6-prompt", "prompt", json!({"message":"Invoke the prescribed original Native Task once."})).await;
            let bridge = if source == "rpc" && mode == "allow" {
                let call = process.until(|frame| frame["type"] == "host_tool_call").await;
                assert_eq!(call["toolName"], actual_name);
                assert_eq!(call["toolCallId"], "g3-f6-external-call");
                assert_ne!(call["id"], call["toolCallId"], "reply must bind the actual bridge pending ID");
                assert_eq!(call["arguments"], child_args);
                let response = json!({"type":"host_tool_result","id":call["id"],
                    "result":{"content":[{"type":"text","text":"f6 actual RPC bridge response"}]}});
                process.send(response.clone()).await;
                json!({"request":call,"response":response})
            } else { Value::Null };
            process.until(|frame| frame["type"] == "agent_end").await;
            let frames = gap_finish(process, &bank, true).await;
            let journals = actual_journals(&bank);
            let children = journals.iter().filter(|journal| journal["init"]["isolated"] == true).collect::<Vec<_>>();
            let root_result = journals.iter().flat_map(|journal| journal["rows"].as_array().unwrap())
                .find(|message| message["role"] == "toolResult" && message["toolCallId"] == "g3-f6-task").unwrap();
            let grants = if mode == "catalog-absent" { Value::Null } else {
                serde_json::from_slice::<Value>(&fs::read(bank.directory.join("task-source-grants.json")).unwrap()).unwrap()
            };
            if mode == "allow" {
                assert_eq!(grants.as_array().unwrap().len(), 1);
                assert_eq!(grants[0]["before"], true);
                assert_eq!(grants[0]["after"], true);
                assert_eq!(root_result["isError"], false);
                assert_eq!(root_result["details"]["taskAccepted"], false);
                assert_eq!(root_result["details"]["results"][0]["runSucceeded"], true);
                assert_eq!(children.len(), 1);
                assert_eq!(children[0]["init"]["restrictToolNames"], false);
                assert!(children[0]["init"]["tools"].as_array().unwrap().iter().any(|tool| tool == actual_name));
                let output = children[0]["rows"].as_array().unwrap().iter()
                    .find(|message| message["role"] == "toolResult" && message["toolCallId"] == "g3-f6-external-call").unwrap();
                assert_eq!(output["isError"], false);
                assert!(output.to_string().contains(if source == "rpc" {
                    "f6 actual RPC bridge response"
                } else { "echo: f6 actual MCP child value" }));
                assert_eq!(up.counts(), [2, 2, 0]);
                let requests = lines(&bank.directory.join("child-a-requests.jsonl"));
                assert!(requests[0]["body"]["tools"].as_array().unwrap().iter()
                    .any(|tool| tool["function"]["name"] == actual_name));
                let cwd = Path::new(children[0]["header"]["cwd"].as_str().unwrap());
                assert!(!cwd.exists(), "normal one-shot close must release the actual external-tool child resource");
            } else {
                assert_eq!(root_result["isError"], true);
                assert!(root_result.to_string().contains(if mode == "deny" {
                    "has no actual parent grant"
                } else { "not exposed by the actual parent" }));
                if mode == "deny" {
                    assert_eq!(grants.as_array().unwrap().len(), 1);
                    assert_eq!(grants[0]["before"], true);
                    assert_eq!(grants[0]["after"], false);
                }
                assert!(children.is_empty(), "whole batch must reject before the valid first child starts");
                assert_eq!(journals.len(), 1, "source denial must not create an extra child transcript");
                assert!(gap_files_named(&bank.home, "owner.json").is_empty());
                assert!(gap_files_named(&bank.home, "baseline.json").is_empty());
                assert!(gap_files_named(&bank.home, "result.json").is_empty());
                assert!(!frames.iter().any(|frame| frame["type"] == "host_tool_call"));
                assert_eq!(up.counts(), [2, 0, 0]);
                assert!(!marker.exists());
            }
            if mode != "catalog-absent" {
                assert_eq!(grants[0]["source"], source);
                assert_eq!(grants[0]["tool"], actual_name);
                assert!(grants[0]["owner_pointer"].as_str().unwrap().starts_with("0x"));
                assert!(grants[0]["source_pointer"].as_str().unwrap().starts_with("0x"));
                assert!(grants[0]["snapshot_pointer"].as_str().unwrap().starts_with("0x"));
                for field in ["advertised_before","advertised_after"] {
                    assert!(grants[0][field].as_array().unwrap().iter().any(|tool| tool == actual_name),
                        "source false must use an advertised exact owner, not a missing-name substitute");
                }
            }
            let server = if source == "mcp" {
                let record: Value = serde_json::from_slice(&fs::read(&mcp_record).unwrap()).unwrap();
                assert_eq!(record["ambient_key_present"], false);
                if mode == "allow" {
                    assert_eq!(record["call"]["name"], "echo");
                    assert_eq!(record["call"]["arguments"], child_args);
                    assert_eq!(fs::read_to_string(&marker).unwrap(), "dispatched");
                } else {
                    assert!(record.get("call").is_none(), "the real MCP connection must not dispatch a denied child");
                }
                record
            } else { Value::Null };
            assert_eq!(before, GapParentSnapshot::capture(&bank, "after-source-grant-task"));
            fs::write(bank.directory.join("gap-native-source-grant-assertions.json"),
                serde_json::to_vec_pretty(&json!({
                    "source":source,"mode":mode,"currentSourceGrants":grants,"bridge":bridge,
                    "mcpServer":server,"frames":frames,"journals":journals,"requests":up.counts(),
                    "exactOwnerNone":"OPEN: catalog absence does not exercise the exact-owner None branch",
                    "foreignOwner":"OPEN: no actual owner substitution authorized or executed"
                })).unwrap()).unwrap();
        }
    }
}
