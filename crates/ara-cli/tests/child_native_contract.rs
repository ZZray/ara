//! Three coarse banks exercise the ordinary binary's production child factory/actor.
//! Controlled HTTP drives model turns; identity, journals, permissions and effects are real.
#![cfg(feature = "test-fixture")]
use ara_testkit::{FakeUpstream, Script, chunks::{text,tool_call,finish,done}};
use serde_json::{Value,json};
use std::{path::{Path,PathBuf},time::Duration};
const BIN:&str=env!("CARGO_BIN_EXE_ara");
struct Bank {root:PathBuf, receipts:Vec<Value>, requests:Vec<Value>}
/// Narrow loopback response gate for the real A/B actor interleaving. B's
/// first response is held until the child process records the actual busy receipt.
struct DispatchReplyGate {url:String,task:tokio::task::JoinHandle<()>}
impl Drop for DispatchReplyGate{fn drop(&mut self){self.task.abort();}}
impl DispatchReplyGate {
    async fn start(target:std::net::SocketAddr,root:PathBuf,held:usize)->Self{
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        let task=tokio::spawn(async move{
            let mut connections=tokio::task::JoinSet::new();let mut index=0;
            loop{tokio::select!{
                incoming=listener.accept()=>{let Ok((mut client,_))=incoming else{break};index+=1;let ordinal=index;let root=root.clone();
                    connections.spawn(async move{
                        if ordinal<=held {
                            tokio::time::timeout(Duration::from_secs(15),async{
                                while !root.join(format!("race-release-{ordinal}")).is_file(){tokio::time::sleep(Duration::from_millis(5)).await;}
                            }).await.expect("B response gate lacked an actual busy receipt");
                        }
                        let mut upstream=tokio::net::TcpStream::connect(target).await.unwrap();
                        let _=tokio::io::copy_bidirectional(&mut client,&mut upstream).await;
                    });
                },
                _=connections.join_next(),if !connections.is_empty()=>{}
            }}
        });Self{url:format!("http://{address}/v1"),task}
    }
}
fn count_named_calls(value:&Value,id:&str)->usize{match value{
    Value::Object(object)=>usize::from(object.get("type").and_then(Value::as_str)==Some("toolCall")&&object.get("id").and_then(Value::as_str)==Some(id))+object.values().map(|value|count_named_calls(value,id)).sum::<usize>(),
    Value::Array(array)=>array.iter().map(|value|count_named_calls(value,id)).sum(),_=>0,
}}
fn journal_call_count(raw:&str,id:&str)->usize{raw.lines().map(|line|count_named_calls(&serde_json::from_str::<Value>(line).unwrap(),id)).sum()}
fn assignment_count(raw:&str,text:&str)->usize{
    fn count(value:&Value,text:&str)->usize{match value{Value::Object(object)=>usize::from(object.get("customType").and_then(Value::as_str)==Some("child-assignment")&&object.get("content").and_then(Value::as_str)==Some(text))+object.values().map(|value|count(value,text)).sum::<usize>(),Value::Array(array)=>array.iter().map(|value|count(value,text)).sum(),_=>0}}
    raw.lines().map(|line|count(&serde_json::from_str::<Value>(line).unwrap(),text)).sum()
}

fn init(tools:&[&str])->Value {json!({"systemPrompt":["Child native fixture base"],"task":"exercise actual child","tools":tools,"agent":"task","readSummarize":false,"restrictToolNames":true,"spawns":"*","outputSchemaMode":"permissive","retryFallback":{"primary":"default","chain":[]},"compactionThreshold":{"thresholdPercent":72.0,"thresholdTokens":0.0}})}
fn answer(body:&str)->Value {json!({"events":[text(body),finish("stop"),done()]})}
fn yield_result(id:&str,data:Value)->Value {json!({"events":[tool_call(0,id,"yield",&json!({"data":data}).to_string()),finish("tool_calls"),done()]})}
fn pool_result(id:&str,key:usize,data:Value)->Value {json!({"events":[tool_call(0,id,"yield",&json!({"key":key,"data":data}).to_string()),finish("tool_calls"),done()]})}
async fn run(label:&str,operations:Vec<Value>,responses:Vec<Value>)->Bank{
    let out=PathBuf::from(std::env::var_os("ARA_TEST_CHILD_ARTIFACT_DIR").expect("Root must bind child artifact evidence directory"));
    let root=out.join(format!("{label}-{}",uuid::Uuid::now_v7()));std::fs::create_dir_all(&root).unwrap();
    let home=root.join("isolated-home");let project=root.join("project");
    for path in [&home,&project]{std::fs::create_dir_all(path).unwrap();}
    let settings=home.join("agent");std::fs::create_dir_all(&settings).unwrap();
    std::fs::write(settings.join("config.yml"),"features:\n  unexpectedStopDetection: none\nretry:\n  enabled: false\n  modelFallback: false\n").unwrap();
    let scenario=root.join("scenario.json");std::fs::write(&scenario,serde_json::to_vec_pretty(&operations).unwrap()).unwrap();
    let up=FakeUpstream::start(serde_json::from_value::<Script>(json!({"responses":responses})).unwrap(),Some(root.join("requests.jsonl"))).await.unwrap();
    if label=="c5-read-summary" {
        std::fs::write(settings.join("config.yml"),"features:\n  unexpectedStopDetection: none\nretry:\n  enabled: false\n  modelFallback: false\nread:\n  defaultLimit: 500\n  summarize:\n    enabled: true\n    minTotalLines: 0\n    minBodyLines: 2\n    unfoldUntil: 0\n    unfoldLimit: 0\n").unwrap();
        let mut source="export function actualSummaryPolicy() {\n".to_owned();
        for index in 0..180 {source.push_str(&format!("  console.log(\"READ_SUMMARY_BODY_SENTINEL_{index:03}\");\n"));}
        source.push_str("  return 42;\n}\n");std::fs::write(project.join("summary-policy.ts"),source).unwrap();
    }
    if label.starts_with("c5-model-") {
        std::fs::write(settings.join("models.yml"),json!({"providers":{"child-policy-fixture":{
            "api":"openai-completions","baseUrl":up.base_url(),"auth":"none","models":[
                {"id":"fake-model","contextWindow":1_000_000,"maxTokens":512,"input":["text"]},
                {"id":"large-model","contextWindow":500_000,"maxTokens":512,"input":["text"]},
                {"id":"small-model","contextWindow":16_000,"maxTokens":512,"input":["text"]}
            ]}}}).to_string()).unwrap();
        std::fs::write(settings.join("config.yml"),"features:\n  unexpectedStopDetection: none\ncompaction:\n  enabled: true\n  methodOrder: [soft]\nretry:\n  enabled: false\n  modelFallback: false\nmodelRoles:\n  worker: missing-child-provider/removed-role-model\n  unauthorized_worker: openai/gpt-4.1\n").unwrap();
    }
    let response_gate=if label.starts_with("c4-dispatch-race-"){Some(DispatchReplyGate::start(up.addr,root.clone(),if label.ends_with("-3"){3}else{1}).await)}else{None};
    let provider_url=response_gate.as_ref().map(|gate|gate.url.clone()).unwrap_or_else(||up.base_url());
    let mut command=tokio::process::Command::new(BIN);
    if label=="c4-dispatch-lost-ack"{command.env("ARA_TEST_CHILD_DROP_ADMITTED_ACK","1");}else{command.env_remove("ARA_TEST_CHILD_DROP_ADMITTED_ACK");}
    for key in ["OPENROUTER_API_KEY","OPENAI_API_KEY","ANTHROPIC_API_KEY","ARA_TEST_API_KEY","ARA_MODEL","ARA_BASE_URL","ARA_TEST_BASE_URL","ARA_TEST_MODEL_ID","OPENROUTER_BASE_URL","ARA_PROVIDER","ARA_AUTH_BROKER_URL","ARA_AUTH_BROKER_TOKEN","ARA_AUTH_BROKER_ACCOUNT_POOL_FILE","ARA_AUTH_BROKER_SNAPSHOT_CACHE","ARA_AUTH_BROKER_SNAPSHOT_TTL_MS","CLAUDE_CONFIG_DIR","COPILOT_HOME","COPILOT_CUSTOM_INSTRUCTIONS_DIRS"]{command.env_remove(key);}
    command.current_dir(&project).env("HOME",&home).env("USERPROFILE",&home).env("ARA_HOME",&home)
        .env("APPDATA",home.join("AppData/Roaming")).env("LOCALAPPDATA",home.join("AppData/Local"))
        .env("ARA_API_KEY","sk-child-native-controlled-fixture").env("ARA_TEST_CHILD_CONTRACT_SCENARIO",&scenario)
        .env("ARA_TEST_CHILD_ARTIFACT_DIR",&root)
        .args(["--mode","rpc","--api","openai-completions","--model","fake-model","--base-url",&provider_url,"--no-skills","--tools","read,write,bash","--compact-threshold","0","--cwd"])
        .arg(&project).arg("--session-dir").arg(home.join("sessions"))
        .stdin(std::process::Stdio::null()).kill_on_drop(true);
    // Keep one complete seed tool cycle (12 estimated raw tokens); budget 1
    // stops at the trailing ToolResult, which cannot itself start a retained cut.
    if label.starts_with("c5-model-"){for key in ["ARA_API_KEY","ARA_TEST_API_KEY","OPENAI_API_KEY","OPENROUTER_API_KEY"]{command.env_remove(key);}command.args(["--provider","child-policy-fixture","--compact-keep-tokens","16","--max-tokens","512"]);}
    let result=tokio::time::timeout(Duration::from_secs(90),command.output()).await.expect("bounded child process bank timed out").unwrap();
    drop(response_gate);
    std::fs::write(root.join("stdout.txt"),&result.stdout).unwrap();std::fs::write(root.join("stderr.txt"),&result.stderr).unwrap();
    std::fs::write(root.join("process.json"),json!({"success":result.status.success(),"code":result.status.code(),"served":up.served()}).to_string()).unwrap();
    assert!(result.status.success(),"{label}: {}",String::from_utf8_lossy(&result.stderr));
    let stdout=String::from_utf8(result.stdout).unwrap();let raw=stdout.lines().find_map(|line|line.strip_prefix("CHILD_FIXTURE_RECEIPTS=")).expect("actual binary fixture receipt");
    let receipts=serde_json::from_str(raw).unwrap();let requests=up.requests.lock().await.clone();
    std::fs::write(root.join("all-requests.json"),serde_json::to_vec_pretty(&requests).unwrap()).unwrap();
    Bank{root,receipts,requests}
}
fn result(bank:&Bank,index:usize)->&Value{&bank.receipts[index]["result"]}
fn tool_names(request:&Value)->Vec<&str>{request["body"]["tools"].as_array().or_else(||request["tools"].as_array()).map(|items|items.iter().filter_map(|item|item["function"]["name"].as_str()).collect()).unwrap_or_default()}
fn body(request:&Value)->&Value {request.get("body").unwrap_or(request)}
fn assert_event_receipts(bank:&Bank,id:&str){let path=bank.root.join(format!("{id}.events.jsonl"));let events=std::fs::read_to_string(path).unwrap();assert!(events.contains("agent_start")&&events.contains("agent_end"),"missing actual process lifecycle: {events}");}
fn actual_file(value:&Value)->&Path{Path::new(value["file"].as_str().unwrap())}

fn assert_journal_history_prefix(before:&str,after:&str){
    assert!(before.ends_with('\n')&&after.ends_with('\n'),"journal rows must retain complete JSONL framing");
    let old=before.split_inclusive('\n').collect::<Vec<_>>();
    let new=after.split_inclusive('\n').collect::<Vec<_>>();
    assert!(new.len()>=old.len(),"restored journal lost original rows");
    for(index,(original,restored))in old.iter().zip(&new).enumerate(){
        if original==restored{continue;}
        assert_eq!(original.ends_with("\r\n"),restored.ends_with("\r\n"),"newline framing changed at row {index}");
        let mut original:Value=serde_json::from_str(original).unwrap();
        let mut restored:Value=serde_json::from_str(restored).unwrap();
        assert_eq!(original["type"],"session_init","only generated init metadata may change JSON member formatting at row {index}");
        assert_eq!(restored["type"],"session_init");
        // These declared f64 fields pass through the exact Native Number domain.
        // Keep every other known/unknown field strict; never round opaque metadata.
        for init in [&mut original,&mut restored] {
            for field in ["thresholdPercent","thresholdTokens"] {
                if let Some(number)=init.get_mut("compactionThreshold").and_then(|pair|pair.get_mut(field)) {
                    assert!(number.is_number(),"compactionThreshold.{field} must remain numeric at row {index}");
                    let native=ara_session::native_wire_from_serde(number).expect("compaction number must project exactly to Native");
                    *number=ara_session::native_checked_serde(&native).expect("compaction Native Number bits must survive JSON projection");
                }
            }
        }
        assert_eq!(original,restored,"complete original init metadata/identity changed at row {index}");
    }
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn child_contract_c1_c4_create_restore_result_and_fault_bank(){
    assert!(ara_session::native_wire_from_serde(&json!(9007199254740993_u64)).is_err(),"journal comparison must reject an inexact Native integer projection");
    let bank=run("c1-c4",vec![
        json!({"op":"create","init":init(&["yield"]),"structured":true,"context":"actual context","planReference":"plan receipt"}),
        json!({"op":"followup","text":"submit first"}),json!({"op":"inspect"}),json!({"op":"close"}),json!({"op":"open"}),
        json!({"op":"followup","text":"submit restored"}),json!({"op":"clear"}),json!({"op":"inspect"}),json!({"op":"close"}),
        json!({"op":"create","alias":"plain","init":init(&[]),"structured":false}),json!({"op":"followup","alias":"plain","text":"unstructured final text"}),json!({"op":"close","alias":"plain"}),
        json!({"op":"create","alias":"fault","init":init(&["yield"]),"structured":true}),json!({"op":"followup","alias":"fault","text":"provider fault"}),json!({"op":"inspect","alias":"fault"}),json!({"op":"close","alias":"fault"}),
        json!({"op":"create","alias":"cancel","init":init(&["yield"]),"structured":true}),json!({"op":"followup","alias":"cancel","text":"cancel real admitted run","cancelAfterMs":150,"expectedError":true}),json!({"op":"inspect","alias":"cancel"}),json!({"op":"close","alias":"cancel"}),
        json!({"op":"append_init","patch":{"isolated":true}}),json!({"op":"open","expectedError":true}),json!({"op":"append_init","patch":{"isolated":false}}),
        json!({"op":"transcript_fault","kind":"missing"}),json!({"op":"open","expectedError":true}),json!({"op":"transcript_fault","kind":"restore"}),json!({"op":"open"}),json!({"op":"close"}),
        json!({"op":"transcript_fault","kind":"truncated"}),json!({"op":"open","expectedError":true}),json!({"op":"transcript_fault","kind":"restore"}),
        json!({"op":"transcript_fault","kind":"wrong-cwd"}),json!({"op":"open","expectedError":true}),json!({"op":"transcript_fault","kind":"restore"}),
        json!({"op":"append_init","patch":{"systemPrompt":"foreign cold restored prompt","fixtureUnknown":{"version":1}},"removeFields":["araChildPromptBase","araChildPromptInputs"]}),
        json!({"op":"open"}),json!({"op":"install","items":[{"id":"foreign-live-item","index":9}]}),json!({"op":"inspect"}),json!({"op":"clear"}),json!({"op":"close"}),json!({"op":"open"}),json!({"op":"inspect"}),json!({"op":"close"})
    ],vec![yield_result("first",json!({"round":1})),yield_result("second",json!({"round":2})),answer("unstructured native text"),json!({"status":400,"body":"{\"error\":{\"message\":\"controlled permanent child failure\"}}"}),json!({"events":[],"end":"hang"})]).await;
    assert_eq!(result(&bank,0)["id"],result(&bank,4)["id"]);assert_eq!(result(&bank,0)["file"],result(&bank,4)["file"]);
    assert_eq!(result(&bank,1)["data"],json!({"round":1}));assert_eq!(result(&bank,5)["data"],json!({"round":2}));
    assert_eq!(result(&bank,1)["accepted"],true);assert_eq!(result(&bank,5)["accepted"],true);
    assert_eq!(result(&bank,10)["data"],"unstructured native text");assert_eq!(result(&bank,10)["accepted"],true);
    assert_eq!(result(&bank,7)["init"]["readSummarize"],false);assert_eq!(result(&bank,7)["init"]["restrictToolNames"],true);
    assert_eq!(result(&bank,7)["init"]["workPoolYieldItems"],Value::Null);
    let first=result(&bank,2)["rawJournal"].as_str().unwrap();let restored=result(&bank,7)["rawJournal"].as_str().unwrap();assert_journal_history_prefix(first,restored);
    assert_eq!(result(&bank,13)["status"],"Failed");assert_eq!(result(&bank,13)["accepted"],false);assert_eq!(bank.receipts[17]["success"],false);
    assert!(actual_file(result(&bank,0)).is_file());assert!(result(&bank,14)["rawJournal"].as_str().unwrap().contains("controlled permanent child failure"));
    assert_event_receipts(&bank,result(&bank,0)["id"].as_str().unwrap());assert_eq!(bank.requests.len(),5);
    for index in [21,24,29,32]{assert_eq!(bank.receipts[index]["success"],false,"cold failure must remain explicit at operation {index}");}
    for index in [26,35,40]{assert_eq!(result(&bank,index)["id"],result(&bank,0)["id"]);assert_eq!(result(&bank,index)["file"],result(&bank,0)["file"]);}
    assert_eq!(result(&bank,36)["systemPrompt"],json!(["foreign cold restored prompt"]));
    assert_eq!(result(&bank,37)["init"]["fixtureUnknown"],json!({"version":1}));
    assert_eq!(result(&bank,37)["init"]["workPoolYieldItems"],json!([{"id":"foreign-live-item","index":9}]));
    assert!(result(&bank,41)["init"].get("workPoolYieldItems").is_none());
    assert_eq!(result(&bank,41)["init"]["systemPrompt"],json!(["foreign cold restored prompt"]));
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn child_contract_c3_pool_drop_waiter_clear_and_reminder_bank(){
    let bank=run("c3",vec![
        json!({"op":"create","init":init(&["yield"]),"structured":true}),
        json!({"op":"drop_install_waiter","items":[{"id":"owned-A","index":7}]}),
        json!({"op":"ordinary_probe","expectedError":true}),json!({"op":"inspect"}),
        json!({"op":"followup","text":"submit pooled item","items":[{"id":"owned-A","index":7}]}),
        json!({"op":"clear"}),json!({"op":"followup","text":"ordinary after clear"}),json!({"op":"inspect"}),json!({"op":"close"}),
        json!({"op":"create","alias":"reminder","init":init(&["yield"]),"structured":true}),
        json!({"op":"followup","alias":"reminder","text":"exercise exactly three reminders"}),json!({"op":"close","alias":"reminder"}),
        json!({"op":"open"}),json!({"op":"journal_path_fault","enable":true}),json!({"op":"install","items":[{"id":"fault-B","index":11}],"expectedError":true}),
        json!({"op":"journal_path_fault","enable":false}),json!({"op":"close"}),json!({"op":"open"}),json!({"op":"followup","text":"recover actual rejected publication"}),json!({"op":"inspect"}),
        json!({"op":"ordinary_probe"}),json!({"op":"wait_idle"}),json!({"op":"inspect"}),json!({"op":"close"}),
        json!({"op":"open"}),json!({"op":"install","items":[{"id":"successful-before-clear","index":23}]}),json!({"op":"inspect"}),
        json!({"op":"journal_path_fault","enable":true}),json!({"op":"clear","expectedError":true}),json!({"op":"journal_path_fault","enable":false}),json!({"op":"close"}),
        json!({"op":"open"}),json!({"op":"inspect"}),json!({"op":"clear"}),json!({"op":"ordinary_probe"}),json!({"op":"wait_idle"}),json!({"op":"inspect"}),json!({"op":"close"}),
        json!({"op":"report_extractor_bank","turns":[
            [[{"type":"text","text":"first report block"},{"type":"text","text":""},{"type":"text","text":"second report block"}]],
            [[{"type":"text","text":"stale old report"}],[{"type":"text","text":"mid-work narration"},{"type":"toolCall","id":"source-work","name":"bash","arguments":{"command":"source-only"}}]],
            [[{"type":"toolCall","id":"source-finalize","name":"yield","arguments":{"type":"result"}}]],
            [[{"type":"text","text":"fresh completed report"}]],
            [[{"type":"toolCall","id":"source-new-finalize","name":"yield","arguments":{"type":"result"}}]]
        ]})
    ],vec![pool_result("pooled",7,json!({"item":"A"})),yield_result("ordinary",json!({"afterClear":true})),answer("first without yield"),answer("second without yield"),answer("third without yield"),yield_result("forced",json!({"forced":true})),yield_result("recovered",json!({"recovered":true})),yield_result("ordinary-wake",json!({"woke":true})),yield_result("ordinary-after-rejected-clear",json!({"recoveredClear":true}))]).await;
    assert_eq!(bank.receipts[2]["success"],false);assert_eq!(result(&bank,1)["dispatchActive"],false);
    assert_eq!(result(&bank,3)["init"]["workPoolYieldItems"],json!([{"id":"owned-A","index":7}]));
    assert_eq!(result(&bank,4)["accepted"],true);assert_eq!(result(&bank,6)["accepted"],true);assert_eq!(result(&bank,10)["accepted"],true);
    assert_eq!(result(&bank,5)["items"],json!([]));assert_eq!(bank.requests.len(),9);
    assert_eq!(body(&bank.requests[5])["tool_choice"]["function"]["name"],"yield");
    let raw=result(&bank,7)["rawJournal"].as_str().unwrap();assert!(raw.contains("pooled")&&raw.contains("ordinary"));
    assert_eq!(bank.receipts[14]["success"],false,"actual journal append must refuse the publication");
    assert_eq!(result(&bank,0)["id"],result(&bank,17)["id"]);assert_eq!(result(&bank,0)["file"],result(&bank,17)["file"]);
    assert_eq!(result(&bank,18)["accepted"],true);assert!(!result(&bank,19)["rawJournal"].as_str().unwrap().contains("fault-B"));
    assert_eq!(result(&bank,20)["admitted"],true);assert_eq!(result(&bank,21)["dispatchActive"],false);
    fn count_calls(value:&Value)->usize{match value{Value::Object(o)=>usize::from(o.get("type").and_then(Value::as_str)==Some("toolCall")&&o.get("id").and_then(Value::as_str)==Some("ordinary-wake"))+o.values().map(count_calls).sum::<usize>(),Value::Array(a)=>a.iter().map(count_calls).sum(),_=>0}}
    let calls:usize=result(&bank,22)["rawJournal"].as_str().unwrap().lines().map(|line|count_calls(&serde_json::from_str::<Value>(line).unwrap())).sum();assert_eq!(calls,1,"ordinary wake after clear must execute once");
    assert_eq!(bank.receipts[28]["success"],false,"failing clear must not journal a replacement empty contract");
    assert_eq!(result(&bank,26)["init"]["workPoolYieldItems"],json!([{"id":"successful-before-clear","index":23}]));
    assert_eq!(result(&bank,32)["init"]["workPoolYieldItems"],result(&bank,26)["init"]["workPoolYieldItems"]);
    assert_eq!(result(&bank,32)["init"]["systemPrompt"],result(&bank,26)["init"]["systemPrompt"]);
    assert_eq!(result(&bank,27)["beforeHash"],result(&bank,29)["afterHash"],"failed clear must preserve the actual last successful transcript bytes");
    assert_eq!(result(&bank,31)["id"],result(&bank,0)["id"]);assert_eq!(result(&bank,31)["file"],result(&bank,0)["file"]);
    assert_eq!(result(&bank,33)["items"],json!([]));assert_eq!(result(&bank,34)["admitted"],true);assert_eq!(result(&bank,35)["dispatchActive"],false);
    assert!(result(&bank,36)["init"].get("workPoolYieldItems").is_none());assert!(result(&bank,36)["rawJournal"].as_str().unwrap().contains("ordinary-after-rejected-clear"));
    assert_eq!(result(&bank,38)["candidates"],json!(["first report block\nsecond report block",null,null,"fresh completed report","fresh completed report"]));
    assert_eq!(result(&bank,38)["typedCandidates"],result(&bank,38)["candidates"]);
    assert_event_receipts(&bank,result(&bank,0)["id"].as_str().unwrap());

    let last_turn=|id:&str|json!({"events":[tool_call(0,id,"yield",r#"{"type":"result"}"#),finish("tool_calls"),done()]});
    let positive=run("c4-last-report-positive",vec![json!({"op":"create","init":init(&["yield"]),"structured":true}),json!({"op":"followup","text":"report then data-less finalize"}),json!({"op":"inspect"}),json!({"op":"close"})],vec![answer("Completed report survives unchanged."),last_turn("pure-report-yield")]).await;
    assert_eq!(result(&positive,1)["accepted"],true);assert_eq!(result(&positive,1)["data"],"Completed report survives unchanged.");assert_eq!(positive.requests.len(),2);
    let mut resumed=init(&["bash","yield"]);resumed["readOnly"]=json!(false);
    let mut stale_responses=vec![answer("Everything is done; nothing left to change."),json!({"events":[text("Actually one more fix."),tool_call(0,"resume-real-work","bash",r#"{"command":"echo child-resumed-work > child-resumed-work.txt"}"#),finish("tool_calls"),done()]})];
    for id in ["empty-finalize-one","empty-finalize-two","empty-finalize-three","empty-finalize-four"]{stale_responses.push(last_turn(id));}
    let stale=run("c4-last-report-work-invalidates",vec![json!({"op":"create","init":resumed,"structured":true}),json!({"op":"followup","text":"report, resume actual work, then data-less finalize"}),json!({"op":"inspect"}),json!({"op":"close"})],stale_responses).await;
    assert_eq!(result(&stale,1)["accepted"],false);assert_ne!(result(&stale,1)["data"],"Everything is done; nothing left to change.");
    assert_ne!(result(&stale,1)["data"],"Actually one more fix.");assert_eq!(result(&stale,1)["status"],"Aborted");
    assert!(stale.root.join("project/child-resumed-work.txt").is_file());assert!(!std::fs::read(stale.root.join("project/child-resumed-work.txt")).unwrap().is_empty());
    let journal=result(&stale,2)["rawJournal"].as_str().unwrap();assert!(journal.contains("resume-real-work")&&journal.contains("empty-finalize-four"));assert_eq!(stale.requests.len(),6);
    std::fs::write(stale.root.join("last-report-boundary.json"),json!({"source":"executor-subagent-reminders.test.ts736-788","realBashEffect":true,"accepted":false,"actualYieldDifference":"real callback rejects empty current report and aborts on fourth empty yield; upstream mock directly emitted a successful useLastTurn receipt","controlledExtractorCase":"c3 operation38 verifies exact invalidate/no-update/newline state independently"}).to_string()).unwrap();

    for collisions in [1,3] {
        let mut responses=Vec::new();for index in 0..collisions{responses.push(yield_result(&format!("actual-B-{}",index+1),json!({"owner":"B","round":index+1})));}
        if collisions==1{responses.push(yield_result("winning-A",json!({"owner":"A"})));}
        let race=run(&format!("c4-dispatch-race-{collisions}"),vec![json!({"op":"create","init":init(&["yield"]),"structured":true}),json!({"op":"dispatch_race","collisions":collisions,"text":"Lost race assignment A"}),json!({"op":"inspect"}),json!({"op":"last_turn_receipt"}),json!({"op":"close"})],responses).await;
        let receipt=result(&race,1);assert_eq!(receipt["attempts"],if collisions==1{2}else{3});assert_eq!(receipt["busyReceipts"].as_array().unwrap().len(),collisions);
        assert_eq!(receipt["outcome"]["succeeded"],collisions==1);assert_eq!(receipt["outcome"]["accepted"],collisions==1);
        let raw=result(&race,2)["rawJournal"].as_str().unwrap();assert_eq!(assignment_count(raw,"Lost race assignment A"),usize::from(collisions==1));
        let mut competing=std::collections::HashSet::new();
        for rejected in receipt["busyReceipts"].as_array().unwrap(){assert_eq!(rejected["aReceipt"]["state"],"BusyNotDispatched");assert_eq!(rejected["aReceipt"]["sessionId"],result(&race,0)["id"]);competing.insert(rejected["aReceipt"]["competingDispatchId"].as_u64().expect("actual B dispatch receipt"));}
        assert_eq!(competing.len(),collisions);
        for index in 0..collisions{assert_eq!(journal_call_count(raw,&format!("actual-B-{}",index+1)),1);}
        if collisions==1{assert_eq!(receipt["outcome"]["data"],json!({"owner":"A"}));assert_eq!(result(&race,3)["state"],"Admitted");assert!(!competing.contains(&result(&race,3)["dispatchId"].as_u64().unwrap()));assert_eq!(journal_call_count(raw,"winning-A"),1);}
        else{assert_eq!(result(&race,3)["state"],"BusyNotDispatched");assert!(receipt["outcome"]["error"].as_str().unwrap().contains("3 total attempts"));}
        assert_eq!(race.requests.len(),collisions+usize::from(collisions==1));
    }
    let unknown=run("c4-dispatch-unknown",vec![json!({"op":"create","init":init(&["yield"]),"structured":true}),json!({"op":"dispatch_race","collisions":0,"closeBeforeSend":true,"text":"Unknown dispatch must never replay"}),json!({"op":"inspect"}),json!({"op":"last_turn_receipt"}),json!({"op":"close"})],Vec::new()).await;
    assert_eq!(result(&unknown,1)["attempts"],1);assert_eq!(result(&unknown,1)["outcome"]["accepted"],false);assert_eq!(result(&unknown,3)["state"],"Unobserved");assert!(result(&unknown,1)["busyReceipts"].as_array().unwrap().is_empty());assert_eq!(unknown.requests.len(),0);assert_eq!(assignment_count(result(&unknown,2)["rawJournal"].as_str().unwrap(),"Unknown dispatch must never replay"),0);
    let lost=run("c4-dispatch-lost-ack",vec![json!({"op":"create","init":init(&["write","yield"]),"structured":true}),json!({"op":"followup","text":"Lost acknowledgement retained owner"}),json!({"op":"last_turn_receipt"}),json!({"op":"inspect"}),json!({"op":"close"})],vec![json!({"events":[tool_call(0,"lost-ack-write","write",r#"{"path":"lost-ack-effect.txt","content":"exactly one actual effect"}"#),finish("tool_calls"),done()]}),yield_result("lost-ack-yield",json!({"ackLost":true}))]).await;
    assert_eq!(result(&lost,1)["accepted"],true);assert_eq!(result(&lost,1)["data"],json!({"ackLost":true}));assert_eq!(result(&lost,2)["state"],"Admitted");assert_eq!(lost.requests.len(),2);assert_eq!(std::fs::read(lost.root.join("project/lost-ack-effect.txt")).unwrap(),b"exactly one actual effect");let raw=result(&lost,3)["rawJournal"].as_str().unwrap();assert_eq!(assignment_count(raw,"Lost acknowledgement retained owner"),1);assert_eq!(journal_call_count(raw,"lost-ack-write"),1);assert_eq!(journal_call_count(raw,"lost-ack-yield"),1);
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn child_contract_c5_live_grants_readonly_restrictions_and_real_effect_bank(){
    let mut readonly=init(&["read","write","yield","rogue-child-tool"]);readonly["readOnly"]=json!(true);
    let bank=run("c5",vec![
        json!({"op":"create","alias":"readonly","init":readonly,"structured":true}),
        json!({"op":"followup","alias":"readonly","text":"read-only must not write"}),json!({"op":"inspect","alias":"readonly"}),json!({"op":"close","alias":"readonly"}),
        json!({"op":"revoke_parent","tool":"write"}),json!({"op":"create","alias":"revoked","init":init(&["write","yield"]),"structured":true}),
        json!({"op":"followup","alias":"revoked","text":"current parent grant must deny write"}),json!({"op":"inspect","alias":"revoked"}),json!({"op":"close","alias":"revoked"})
    ],vec![
        json!({"events":[tool_call(0,"readonly-write","write",r#"{"path":"readonly-must-not-exist.txt","content":"forbidden"}"#),finish("tool_calls"),done()]}),yield_result("readonly-result",json!({"denied":true})),
        json!({"events":[tool_call(0,"revoked-write","write",r#"{"path":"revoked-must-not-exist.txt","content":"forbidden"}"#),finish("tool_calls"),done()]}),yield_result("revoked-result",json!({"denied":true}))
    ]).await;
    assert_eq!(result(&bank,1)["accepted"],true);assert_eq!(result(&bank,6)["accepted"],true);assert_eq!(result(&bank,4)["currentGrant"],false);
    assert!(!bank.root.join("project/readonly-must-not-exist.txt").exists());assert!(!bank.root.join("project/revoked-must-not-exist.txt").exists());
    let names=tool_names(&bank.requests[0]);assert!(names.contains(&"yield")&&names.contains(&"read"));assert!(!names.contains(&"write")&&!names.contains(&"rogue-child-tool")&&!names.contains(&"lsp"));
    let actual=result(&bank,2)["init"]["tools"].as_array().unwrap();assert_eq!(actual.iter().filter_map(Value::as_str).collect::<Vec<_>>(),names);
    assert_eq!(result(&bank,2)["init"]["readOnly"],true);assert_eq!(result(&bank,2)["init"]["readSummarize"],false);
    let raw=result(&bank,7)["rawJournal"].as_str().unwrap();assert!(raw.contains("revoked-write")&&raw.contains("revoked-result"));
    std::fs::write(bank.root.join("effects.json"),json!({"readonlyFileExists":false,"revokedFileExists":false,"parentGrant":false}).to_string()).unwrap();
    assert_event_receipts(&bank,result(&bank,5)["id"].as_str().unwrap());assert_eq!(bank.requests.len(),4);
    let positive=run("c5-positive",vec![json!({"op":"create","init":init(&["write","yield"]),"structured":true}),json!({"op":"followup","text":"write actual owned child artifact"}),json!({"op":"inspect"}),json!({"op":"close"})],vec![
        json!({"events":[tool_call(0,"allowed-child-write","write",r#"{"path":"owned-child.txt","content":"owned child verified"}"#),finish("tool_calls"),done()]}),yield_result("allowed-child-yield",json!({"written":true}))
    ]).await;
    assert_eq!(std::fs::read(positive.root.join("project/owned-child.txt")).unwrap(),b"owned child verified");assert_eq!(result(&positive,1)["accepted"],true);assert_eq!(positive.requests.len(),2);
    let journal=result(&positive,2)["rawJournal"].as_str().unwrap();assert!(journal.contains("allowed-child-write")&&journal.contains("allowed-child-yield"));
    let mut full_read=init(&["read","yield"]);full_read["readOnly"]=json!(true);
    let mut summary_read=full_read.clone();summary_read["readSummarize"]=json!(true);
    let read=run("c5-read-summary",vec![
        json!({"op":"create","alias":"full","init":full_read,"structured":true}),json!({"op":"followup","alias":"full","text":"Read actual summary-policy.ts with child summaries disabled"}),json!({"op":"inspect","alias":"full"}),json!({"op":"close","alias":"full"}),
        json!({"op":"create","alias":"summary","init":summary_read,"structured":true}),json!({"op":"followup","alias":"summary","text":"Read the same file with inherited summaries enabled"}),json!({"op":"inspect","alias":"summary"}),json!({"op":"close","alias":"summary"})
    ],vec![
        json!({"events":[tool_call(0,"full-read","read",r#"{"path":"summary-policy.ts"}"#),finish("tool_calls"),done()]}),yield_result("full-read-yield",json!({"read":true})),
        json!({"events":[tool_call(0,"summary-read","read",r#"{"path":"summary-policy.ts"}"#),finish("tool_calls"),done()]}),yield_result("summary-read-yield",json!({"read":true}))
    ]).await;
    let full_journal=result(&read,2)["rawJournal"].as_str().unwrap();let summary_journal=result(&read,6)["rawJournal"].as_str().unwrap();
    assert!(full_journal.contains("READ_SUMMARY_BODY_SENTINEL_120"));assert!(!full_journal.contains("elidedSpans"));
    assert!(summary_journal.contains("elidedSpans"));assert!(!summary_journal.contains("READ_SUMMARY_BODY_SENTINEL_120"));
    assert_eq!(result(&read,1)["accepted"],true);assert_eq!(result(&read,5)["accepted"],true);assert_eq!(read.requests.len(),4);
    std::fs::write(read.root.join("read-policy-observation.json"),json!({"sameSource":read.root.join("project/summary-policy.ts"),"disabledBodySentinelPresent":true,"enabledSummaryDetailsPresent":true,"disabledChildId":result(&read,0)["id"],"enabledChildId":result(&read,4)["id"]}).to_string()).unwrap();

    // Same durable history and percentage, two actual cold model selections. A fixed
    // startup-derived threshold would suppress compaction in the small-window arm.
    for small in [false,true] {
        let model=if small{"small-model"}else{"large-model"};
        let mut contract=init(&["yield"]);contract["compactionThreshold"]=json!({"thresholdPercent":50.0,"thresholdTokens":0.0});
        let seed="old source facts ".repeat(1100);
        let operations=vec![json!({"op":"create","init":contract,"structured":true}),
            json!({"op":"followup","text":format!("first historical source {seed}")}),json!({"op":"followup","text":format!("second historical source {seed}")}),json!({"op":"followup","text":format!("third historical source {seed}")}),json!({"op":"inspect"}),json!({"op":"close"}),
            json!({"op":"append_init","patch":{"modelRole":"worker","resolvedModel":format!("child-policy-fixture/{model}")}}),json!({"op":"open"}),json!({"op":"followup","text":"continue after current model switch"}),json!({"op":"inspect"}),json!({"op":"close"}),
            json!({"op":"append_init","patch":{"modelRole":"worker","resolvedModel":"missing-saved-provider/removed-saved-model"}}),json!({"op":"open","expectedError":true}),
            json!({"op":"append_init","patch":{"resolvedModel":format!("child-policy-fixture/{model}")}}),json!({"op":"open"}),json!({"op":"inspect"}),json!({"op":"close"})];
        let mut responses=vec![yield_result("seed-one",json!({"round":1})),yield_result("seed-two",json!({"round":2})),yield_result("seed-three",json!({"round":3}))];
        if small{
            // The16000-token model has the original2000-token summary-input floor.
            // Two oversized custom-message entries and their two complete
            // assistant/tool-result groups form four source-defined summary windows; the third custom turn is retained.
            for _ in 0..4 {responses.push(answer("Actual child summary after smaller current model selection."));}
        }
        responses.push(yield_result("after-model-switch",json!({"continued":true})));
        let switched=run(&format!("c5-model-{model}"),operations,responses).await;
        assert_eq!(result(&switched,0)["id"],result(&switched,7)["id"]);assert_eq!(result(&switched,0)["file"],result(&switched,7)["file"]);
        assert_eq!(result(&switched,9)["init"]["resolvedModel"],format!("child-policy-fixture/{model}"));assert_eq!(result(&switched,8)["accepted"],true);
        let path=switched.root.join(format!("{}.events.jsonl",result(&switched,0)["id"].as_str().unwrap()));let events=std::fs::read_to_string(path).unwrap();
        assert_eq!(events.contains("auto_compaction_start"),small,"current-model percentage threshold must distinguish 500000 vs16000 windows");
        let journal=result(&switched,9)["rawJournal"].as_str().unwrap();
        assert_eq!(journal.lines().any(|line|serde_json::from_str::<Value>(line).is_ok_and(|row|row["type"]=="compaction")),small);
        if small {assert!(journal.contains("Actual child summary after smaller current model selection."));assert_eq!(switched.requests.len(),8);
            for request in &switched.requests[3..7] {
                assert_eq!(body(request)["model"],"small-model");
                assert!(body(request)["tools"].as_array().is_none_or(Vec::is_empty));
                let messages=body(request)["messages"].as_array().unwrap();
                assert_eq!(messages.len(),2);
                assert!(messages[0]["content"].as_str().unwrap().contains("Summarize"));
                assert!(messages[1]["content"].as_str().unwrap().contains("<conversation>"));
            }
            assert!(tool_names(&switched.requests[7]).contains(&"yield"));}
        else{assert_eq!(switched.requests.len(),4);}
        assert_eq!(body(switched.requests.last().unwrap())["model"],model);
        assert_eq!(result(&switched,9)["init"]["modelRole"],"worker");assert_eq!(switched.receipts[12]["success"],false);
        assert!(switched.receipts[12]["error"].as_str().unwrap().contains("No child role or saved model resolved"));
        assert_eq!(result(&switched,14)["id"],result(&switched,0)["id"]);assert_eq!(result(&switched,14)["file"],result(&switched,0)["file"]);
    }

    // Existing model but no current auth is different from an absent role model.
    // Generic launch keys are removed for this arm; saved provider is explicit auth:none.
    let auth=run("c5-model-auth-fallback",vec![
        json!({"op":"create","init":init(&["yield"]),"structured":true}),json!({"op":"followup","text":"seed valid saved-model transcript"}),json!({"op":"close"}),
        json!({"op":"append_init","patch":{"modelRole":"unauthorized_worker","resolvedModel":"child-policy-fixture/fake-model"}}),json!({"op":"open"}),json!({"op":"followup","text":"existing unauthorized role must use saved authorized model"}),json!({"op":"inspect"}),json!({"op":"close"}),
        json!({"op":"append_init","patch":{"resolvedModel":"openai/gpt-4.1"}}),json!({"op":"open","expectedError":true}),
        json!({"op":"append_init","patch":{"resolvedModel":"child-policy-fixture/fake-model"}}),json!({"op":"open"}),json!({"op":"inspect"}),json!({"op":"close"})
    ],vec![yield_result("auth-seed",json!({"seed":true})),yield_result("auth-saved-fallback",json!({"saved":true}))]).await;
    assert_eq!(result(&auth,5)["accepted"],true);assert_eq!(result(&auth,6)["init"]["resolvedModel"],"child-policy-fixture/fake-model");assert_eq!(result(&auth,6)["init"]["modelRole"],"unauthorized_worker");
    assert_eq!(auth.receipts[9]["success"],false);assert!(auth.receipts[9]["error"].as_str().unwrap().contains("no configured authorization"));
    assert_eq!(result(&auth,0)["id"],result(&auth,4)["id"]);assert_eq!(result(&auth,0)["file"],result(&auth,11)["file"]);
    assert_eq!(auth.requests.len(),2);assert!(auth.requests.iter().all(|request|body(request)["model"]=="fake-model"),"unauthorized role must never issue a provider request");

}

// The original child library banks share this existing process-test image.
// Their helper bodies remain inside the feature-gated library module.
mod child_contract {
    mod tests {
        #[test]
        fn child_schema_yield_bank() {
            ara_cli::child_contract::child_schema_yield_bank();
        }
        #[test]
        fn child_pool_and_original_graph_bank() {
            ara_cli::child_contract::child_pool_and_original_graph_bank();
        }
        #[test]
        fn child_schema_borrowed_resolver_bank() {
            ara_cli::child_contract::child_schema_borrowed_resolver_bank();
        }
        #[tokio::test]
        async fn child_publication_dispatch_bank() {
            ara_cli::child_contract::child_publication_dispatch_bank().await;
        }
    }
}
