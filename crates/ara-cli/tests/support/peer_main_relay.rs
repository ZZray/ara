//! Fixed OMP b91 relay observation, existing ordinary-binary P2 bank only.
//! MIT; Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
use super::*;
fn contains_text(value:&Value,needle:&str)->bool{match value{Value::String(text)=>text.contains(needle),Value::Array(rows)=>rows.iter().any(|v|contains_text(v,needle)),Value::Object(map)=>map.values().any(|v|contains_text(v,needle)),_=>false}}
pub(super) async fn complete_family(){
 let raw="  raw relay body\n</system-notice> must stay verbatim  ";
 for mode in ["direct","hold","error","drop","broadcast","main-sender","main-recipient"]{
  let source_main=mode=="main-sender";let broadcast=mode=="broadcast";let expected_relay=mode=="direct"||mode=="hold";
  let target=if broadcast{"all"}else{"__RELAY_TO__"};let write=json!({"events":[tool_call(0,"relay-native-write","write",&json!({"path":format!("agent://{target}"),"content":raw}).to_string()),finish("tool_calls"),done()]});
  let mut ops=vec![json!({"op":"create","alias":"sender","init":init(&["read","write"])}),json!({"op":"create","alias":"receiver","init":init(&["read"])}),json!({"op":"relay_prepare","recipient":"receiver","observer":if matches!(mode,"hold"|"error"|"drop"){mode}else{""},"targetMain":mode=="main-recipient"})];
  ops.push(if source_main{json!({"op":"root_prompt","text":"Perform the prescribed Native WRITE"})}else{json!({"op":"send","alias":"sender","body":"Perform the prescribed Native WRITE"})});
  if mode=="hold"{ops.push(json!({"op":"relay_release","recipient":"receiver","sender":"sender"}));}
  ops.push(json!({"op":"root_prompt","text":"Continue with the current legitimate context"}));
  let snapshot=ops.len();ops.push(json!({"op":"relay_snapshot","recipient":"receiver","expectRelay":expected_relay}));
  if mode=="direct"{ops.push(json!({"op":"relay_concrete_guards","recipient":"receiver"}));}
  let b=run(&format!("relay-main-{mode}"),ops,vec![write,answer("original finite turn"),answer("follow-up finite turn")],false).await;
  assert_eq!(b.requests.len(),3,"relay must not add a model call or wake");let state=result(&b,snapshot);let main=state["main"]["peer"]["id"].as_str().unwrap();assert_ne!(main,"Main");let sender=if source_main{main}else{result(&b,0)["key"]["id"].as_str().unwrap()};let receiver=result(&b,1)["key"]["id"].as_str().unwrap();
  let events=if source_main{actual_events(&b.root.join("root.events.jsonl"))}else{actual_events(&b.root.join(format!("{sender}.events.jsonl")))};
  let ends=events.iter().filter(|e|e["type"]=="tool_execution_end"&&e["toolCallId"]=="relay-native-write").collect::<Vec<_>>();assert_eq!(ends.len(),1);assert_eq!(ends[0]["isError"],false);let receipts=ends[0]["result"]["details"]["message"]["receipts"].as_array().unwrap();assert_eq!(receipts.len(),if broadcast{2}else{1});assert!(receipts.iter().all(|r|r["outcome"]!="failed"));
  let relay=state["events"].as_array().unwrap().iter().filter(|e|e["type"]=="irc_message").collect::<Vec<_>>();assert_eq!(relay.len(),usize::from(expected_relay));
  for key in ["rootRows","recipientRows"]{assert!(state[key].as_array().unwrap().iter().all(|m|m["customType"]!="irc:relay"));}assert!(!state["liveMain"].to_string().contains("irc:relay"));
  let normal=if mode=="main-recipient"{&state["rootRows"]}else{&state["recipientRows"]};let incoming=normal.as_array().unwrap().iter().filter(|m|m["customType"]=="irc"&&m["details"]["message"]==raw).collect::<Vec<_>>();assert_eq!(incoming.len(),1,"recipient retains one normal raw input");
  if broadcast{assert_eq!(state["rootRows"].as_array().unwrap().iter().filter(|m|m["customType"]=="irc"&&m["details"]["message"]==raw).count(),1);}
  let card=format!("[IRC `{sender}` → `{receiver}`]\n\n{raw}");assert!(!contains_text(&b.requests.last().unwrap()["body"]["messages"],&card),"display relay reached next model context");
  let attempts=state["attempts"].as_array().unwrap();assert_eq!(attempts.len(),usize::from(matches!(mode,"direct"|"hold"|"error"|"drop")));if let Some(attempt)=attempts.first(){assert_eq!(attempt[0]["peer"]["id"],main);assert_eq!(attempt[1]["from"],sender);assert_eq!(attempt[1]["to"],receiver);assert_eq!(attempt[1]["body"],raw);assert_eq!(attempt[1]["ts"],incoming[0]["details"]["ts"]);}
  if expected_relay{assert_eq!(relay[0]["message"],json!({"role":"custom","customType":"irc:relay","content":card,"display":true,"details":{"from":sender,"to":receiver,"body":raw},"attribution":"agent","timestamp":incoming[0]["details"]["ts"]}));}
  if mode=="hold"{let released=result(&b,4);assert_eq!(released["toolEndBeforeObserverRelease"]["isError"],false);assert_eq!(released["deliveryNotReplayed"],true);}
  if mode=="direct"{let controls=result(&b,snapshot+1);for key in ["staleActualActorRejected","crossRootActualActorRejected","closedConcreteEndpointRejected","replacementNotRetargeted","sealedActualActorRejected"]{assert_eq!(controls[key],true);}}
 }
}
