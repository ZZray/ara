//! Consolidated target semantic/wire regressions in the existing CLI lib image.
//! OMP b91db66e tools-native-edit/native host-edit/encode source contracts.
use super::{
    native_view,
    tsp_backend::{Backend, Node, Paint, Surface},
    tsp_wire::{self, InputDecoder, Packet},
    types::*,
};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
fn collect<'a>(node: &'a Node, kind: &str, out: &mut Vec<&'a Node>) {
    if node.kind == kind {
        out.push(node)
    }
    for child in &node.children {
        collect(child, kind, out)
    }
}
fn hello(credits: usize) -> tsp_wire::Message {
    tsp_wire::Message {
        verb: 'r',
        value: json!({"r":"hello","v":1,"term":"controlled-peer","kinds":["col","tool","section","diff","text","row","icon","spinner","editor","rows"],"credits":credits,"apc":37,"cols":80}),
    }
}
fn wire_messages(wire: &str) -> Vec<(char, Value)> {
    let mut out = vec![];
    let mut chunks = std::collections::HashMap::<(char, String), String>::new();
    for frame in wire.split("\x1b_tsp;").skip(1) {
        let frame = frame.split("\x1b\\").next().unwrap();
        let mut parts = frame.split(';');
        let verb = parts.next().unwrap().chars().next().unwrap();
        let mut body = parts.collect::<Vec<_>>();
        let mut params = std::collections::HashMap::new();
        while body.len() > 1
            && body[0].split_once('=').is_some_and(|(key, _)| {
                !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            })
        {
            let (k, v) = body.remove(0).split_once('=').unwrap();
            params.insert(k, v);
        }
        let mut text = body.join(";");
        if let Some(id) = params.get("c") {
            let key = (verb, (*id).into());
            let previous = chunks.remove(&key).unwrap_or_default();
            text = previous + &text;
            if params.get("m") == Some(&"1") {
                chunks.insert(key, text);
                continue;
            }
        }
        out.push((verb, serde_json::from_str(&text).unwrap()));
    }
    out
}
#[test]
fn target_native_edit_semantics_and_original_graph_owner() {
    let args = EditRenderArgs {
        edits: Some(vec![
            EditEntry {
                path: Some("/tmp/a.ts".into()),
                ..Default::default()
            },
            EditEntry {
                path: Some("/tmp/b.py".into()),
                ..Default::default()
            },
        ]),
        ..Default::default()
    };
    let result = EditResult {
        details: Some(EditDetails {
            per_file_results: Some(vec![
                EditDetails {
                    path: Some("/tmp/a.ts".into()),
                    diff: WireText(" 1|keep\n-2|old\n+2|new\n+3|more".into()),
                    ..Default::default()
                },
                EditDetails {
                    path: Some("/tmp/b.py".into()),
                    diff: WireText("-7|gone".into()),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let n = native_view::edit(
        &args,
        Some(&result),
        RenderOptions::default(),
        &EditRenderContext::default(),
    );
    assert_eq!(n.props["title"], "Edit");
    assert_eq!(n.props["target"], "2 files");
    assert_eq!(n.props["meta"], json!(["+2 −2"]));
    assert_eq!(n.children.len(), 2);
    for section in &n.children {
        assert_eq!(section.kind, "section");
        let mut diffs = vec![];
        collect(section, "diff", &mut diffs);
        assert_eq!(diffs.len(), 1);
        assert!(diffs[0].props.get("path").is_none());
    }
    let messages = (0..5)
        .map(|n| format!("src/a.ts:{}:1 [warning] unused {n}", n + 10).into())
        .chain([
            "src/a.ts:40:3 [error] [ts] Type mismatch (2322)".into(),
            "src/a.ts:2:1 [info] note".into(),
        ])
        .collect();
    let result = EditResult {
        details: Some(EditDetails {
            diagnostics: Some(Diagnostics {
                errored: true,
                messages,
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    let n = native_view::edit(
        &EditRenderArgs::default(),
        Some(&result),
        RenderOptions::default(),
        &EditRenderContext::default(),
    );
    let d = n.children.iter().find(|n| n.key == "diagnostics").unwrap();
    assert_eq!(d.children.len(), 6);
    assert_eq!(d.children[0].children[0].props["name"], "x-circle");
    assert_eq!(d.children[0].children[1].props["spans"][0]["t"], "40:3");
    assert_eq!(
        d.children[0].children[2].props["spans"][0]["t"],
        "Type mismatch ts 2322"
    );
    assert_eq!(d.children[5].props["spans"][0]["t"], "+2 more");
    let realm = crate::native_realm::NativeRealm::prepare().unwrap();
    let callbacks = realm.begin_display_callbacks().unwrap();
    let runtime = callbacks.runtime().unwrap();
    let g = super::graph_renderer::DisplayGraph(&realm.values, &runtime);
    let root = realm
        .values
        .heap()
        .import_serde(&json!({"path":"src/native.rs","previewDiff":"-1|before\n+1|after"}))
        .unwrap();
    let mut tool = super::tool_execution::ToolExecutionComponent::new(
        "native-real-graph".into(),
        "edit".into(),
        EditRenderArgs::default(),
        ara_edit::EditMode::Replace,
        std::sync::Arc::new(super::theme::Theme::dark()),
    );
    tool.graph_args_root = Some(root);
    tool.set_native(true);
    assert!(tool.rebuild_in(&g).is_empty());
    assert!(
        tool.graph.is_none(),
        "native must bypass classic Graph renderer constructor"
    );
    let n = tool.describe_native(Some(&g), 1).unwrap().unwrap();
    assert_eq!(n.kind, "tool");
    assert!(n.props["target"].as_str().is_some_and(|s| s.contains("native.rs")));
    // An empty preview is false in the source branch; nullish selection inside
    // a selected branch remains distinct from the branch's truthiness gate.
    let preview=native_view::edit(&EditRenderArgs{preview_diff:Some("".into()),new_text:Some("fallback code".into()),..Default::default()},None,RenderOptions::default(),&EditRenderContext::default());
    let mut code=vec![];collect(&preview,"code",&mut code);assert_eq!(code.len(),1);assert_eq!(code[0].props["text"],"fallback code");
    let nullish=native_view::edit(&EditRenderArgs{preview_diff:Some("".into()),diff:Some("-1|old\n+1|new".into()),op:Some(Operation::Update),..Default::default()},None,RenderOptions::default(),&EditRenderContext::default());
    let mut diffs=vec![];collect(&nullish,"diff",&mut diffs);assert_eq!(diffs.len(),1);assert!(diffs[0].props["hunks"].as_array().unwrap().is_empty(),"truthy diff/op selects the branch, then empty preview wins nullish selection");
    // Actual original Graph getters must follow source branches. Unobserved
    // fields throw a sentinel, so exporting/projecting the whole object fails.
    use ara_rpc::value_heap::{CompiledFunctionSpec,CompiledFunctionForm,ValueSnapshot,ValueError,PropertyDescriptor};
    static THROW:CompiledFunctionSpec=CompiledFunctionSpec{source:"unified AST renderer unobserved field sentinel",name:"unobserved",length:0,form:CompiledFunctionForm::Ordinary,capture_count:1,body:|_,captures,_,_,_|Err(ValueError::Thrown(captures[0].clone()))};
    let heap=realm.values.heap();let sentinel=heap.object().unwrap();
    let poison=|root:&ara_rpc::value_heap::ValueHandle,key:&str|{
        let getter=heap.fresh_compiled_function(&THROW,&[sentinel.clone()]).unwrap();let ValueSnapshot::Function(getter)=heap.snapshot(&getter).unwrap() else{panic!("compiled getter")};
        heap.define_own(root,key.into(),PropertyDescriptor::Accessor{get:Some(getter),set:None,enumerable:true,configurable:true},g.runtime()).unwrap();
    };
    let ast_args=heap.import_serde(&json!({"ops":[{"pat":"foo($A)"}],"paths":[]})).unwrap();let ops=heap.get(&ast_args,&"ops".into(),g.runtime()).unwrap();let op=heap.get(&ops,&"0".into(),g.runtime()).unwrap();poison(&op,"out");
    let pending=native_view::ast_graph(&g,&"ast_edit".into(),&ast_args,None,true).unwrap().unwrap();assert_eq!(pending.props["target"],"foo($A)");
    let error=heap.import_serde(&json!({"isError":true,"content":[{"type":"text","text":"expected error"}],"details":{}})).unwrap();let details=heap.get(&error,&"details".into(),g.runtime()).unwrap();poison(&details,"parseErrors");poison(&ast_args,"paths");
    let failed=native_view::ast_graph(&g,&"ast_edit".into(),&ast_args,Some(&error),true).unwrap().unwrap();assert_eq!(failed.props["tone"],"error");
    let zero=heap.import_serde(&json!({"isError":false,"details":{"parseErrors":[],"totalReplacements":0}})).unwrap();let details=heap.get(&zero,&"details".into(),g.runtime()).unwrap();for key in ["parseErrorsTotal","displayContent","filesTouched","applied"]{poison(&details,key);}poison(&zero,"content");
    let empty=native_view::ast_graph(&g,&"ast_edit".into(),&ast_args,Some(&zero),true).unwrap().unwrap();assert_eq!(empty.props["meta"],json!(["0 replacements"]));
    let done=heap.import_serde(&json!({"isError":false,"details":{"parseErrors":[],"totalReplacements":1,"displayContent":"# a.ts (1 replacement)\n-1│foo(a)\n+1│bar(a)","filesTouched":1,"applied":true}})).unwrap();poison(&done,"content");let details=heap.get(&done,&"details".into(),g.runtime()).unwrap();poison(&details,"parseErrorsTotal");
    let success=native_view::ast_graph(&g,&"ast_edit".into(),&ast_args,Some(&done),true).unwrap().unwrap();assert_eq!(success.props["target"],"a.ts");
    // The selected field still propagates the exact original thrown value.
    poison(&details,"parseErrors");assert!(matches!(native_view::ast_graph(&g,&"ast_edit".into(),&ast_args,Some(&done),true),Err(ValueError::Thrown(reason)) if reason.same_identity(&sentinel)));
    tool.dispose();
    assert!(tool.describe_native(Some(&g), 1).unwrap().is_none());
    let ast = native_view::ast(
        &json!({"ops":[{"pat":"old(  $A )"}]}),
        Some(
            &json!({"content":[],"details":{"totalReplacements":1,"filesTouched":1,"filesSearched":1,"applied":false,"displayContent":"# src/\n## a.ts (1 replacement)\n-1│old(a)\n+1│new(a)"}}),
        ),
    );
    assert_eq!(ast.props["target"], "src/a.ts");
    assert_eq!(ast.props["badges"][0]["text"], "proposed");
    let mut diffs = vec![];
    collect(&ast, "diff", &mut diffs);
    assert_eq!(diffs.len(), 1);
    assert!(diffs[0].props["text"].as_str().unwrap().contains("@@ -1 +1 @@"));
}
#[test]
fn target_tsp_utf8_chunks_apc_osc_and_input_owner() {
    let value = json!({"r":"hello","v":1,"term":"字节🦀k=v;tail","kinds":["tool"]});
    let mut chunk = 0;
    let encoded = tsp_wire::encode('r', &value, 7, &mut chunk);
    assert!(encoded.matches(";c=").count() > 1);
    let osc = encoded.replace("\x1b_tsp;", "\x1b]877;tsp;").replace("\x1b\\", "\x07");
    for data in [encoded.as_bytes(), osc.as_bytes()] {
        for split in 0..=data.len() {
            let mut decoder = InputDecoder::default();
            decoder.push(&data[..split]);
            decoder.push(&data[split..]);
            match decoder.pop() {
                Some(Packet::Tsp(message)) => assert_eq!(message.value, value),
                other => panic!("missing complete message at split {split}: {other:?}"),
            }
            assert!(decoder.pop().is_none());
        }
    }
    let mut decoder = InputDecoder::default();
    for byte in b"a\x1b[200~paste\x1b[201~\x03\x1b[?1;2c" {
        decoder.push(&[*byte])
    }
    assert!(
        matches!(decoder.pop(),Some(Packet::Event(crossterm::event::Event::Key(k))) if k.code==crossterm::event::KeyCode::Char('a'))
    );
    assert!(matches!(decoder.pop(),Some(Packet::Event(crossterm::event::Event::Paste(p)))if p=="paste"));
    assert!(
        matches!(decoder.pop(),Some(Packet::Event(crossterm::event::Event::Key(k)))if k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)&&k.code==crossterm::event::KeyCode::Char('c'))
    );
    assert!(matches!(decoder.pop(), Some(Packet::Da1)));
    for split in 0..=3 {
        let bytes=b"\x1b\xc3\xa9";let mut decoder=InputDecoder::default();decoder.push(&bytes[..split]);decoder.push(&bytes[split..]);
        assert!(matches!(decoder.pop(),Some(Packet::Event(crossterm::event::Event::Key(key))) if key.code==crossterm::event::KeyCode::Char('é')&&key.modifiers==crossterm::event::KeyModifiers::ALT));assert!(decoder.pop().is_none());
    }
    let mut decoder=InputDecoder::default();for byte in b"\x1b\xc3\xa9"{decoder.push(&[*byte]);}
    assert!(matches!(decoder.pop(),Some(Packet::Event(crossterm::event::Event::Key(key))) if key.code==crossterm::event::KeyCode::Char('é')&&key.modifiers==crossterm::event::KeyModifiers::ALT));
    // Windows native UnicodeChar uses this exact helper after UTF16 assembly.
    let mut decoder=InputDecoder::default();decoder.native_character('\r',crossterm::event::KeyModifiers::SHIFT);let Some(Packet::Event(crossterm::event::Event::Key(key)))=decoder.pop()else{panic!("shift enter")};assert!(key.modifiers.contains(crossterm::event::KeyModifiers::SHIFT));
    let mut draft=super::input::InputBuffer::default();draft.insert("first");draft.key(key,true);assert_eq!(draft.draft,"first\n");assert!(draft.result.is_none(),"Shift+Enter cannot submit");
    decoder.native_character('É',crossterm::event::KeyModifiers::ALT|crossterm::event::KeyModifiers::SHIFT);assert!(matches!(decoder.pop(),Some(Packet::Event(crossterm::event::Event::Key(key))) if key.code==crossterm::event::KeyCode::Char('É')&&key.modifiers==crossterm::event::KeyModifiers::ALT|crossterm::event::KeyModifiers::SHIFT));
    let mut input = super::input::InputBuffer::default();
    input.insert("a🦀b");
    input.native_edit(1, 3, "中", 2, 4);
    assert_eq!(input.draft, "a中b");
    input.native_edit(0, 1, "stale", 1, 999);
    assert_eq!(input.draft, "a中b");
    input.native_undo();
    assert_eq!(input.draft, "a🦀b");
    input.native_send("submitted", true);
    assert_eq!(input.result.take().unwrap().unwrap(), Some("submitted\n".into()));
}
#[test]
fn target_tsp_credit_stall_reconcile_close_and_revocation() {
    let now = Instant::now();
    let mut b = Backend::default();
    b.expect_hello_for_test(now);
    b.message(hello(1));
    let surface = |value: &str| Surface {
        main: vec![Node::new("stable", "text", json!({"spans":[{"t":value}]}), vec![])],
        ..Default::default()
    };
    assert!(matches!(b.paint(surface("one"), now), Paint::Native));
    let first = b.take_output();
    let rows=vec!["existing one".into(),"existing two".into()];let output=super::driver::native_frame_output(&rows,&Paint::Native,&first);
    assert!(output.starts_with("\r\x1b[1A\x1b[J"));assert!(output.find("\x1b[J").unwrap()<output.find("\x1b_tsp;o").unwrap());assert!(!output.contains("\x1b[3J"));
    assert_eq!(super::driver::native_frame_output(&[],&Paint::Native,&first),first,"native repaint with cleared ANSI cache does not erase native surface");
    assert_eq!(super::driver::native_frame_output(&rows,&Paint::Rows,"query"),"query","only the native transition erases row paint");

    assert!(wire_messages(&first).iter().any(|(v, _)| *v == 'o'));
    assert!(wire_messages(&first).iter().any(|(v, _)| *v == 'f'));
    assert!(matches!(
        b.paint(surface("two"), now + Duration::from_millis(1)),
        Paint::Native
    ));
    assert!(b.take_output().is_empty(), "second frame must wait for credit");
    b.message(tsp_wire::Message {
        verb: 'e',
        value: json!({"ev":"ack","sf":"ara-session","s":1}),
    });
    b.paint(surface("latest"), now + Duration::from_millis(2));
    let second = b.take_output();
    let messages = wire_messages(&second);
    assert!(messages.iter().any(|(v, _)| *v == 'f'));
    let second = serde_json::to_string(&messages).unwrap();
    assert!(second.contains("latest"));
    assert!(!second.contains("two"));
    b.paint(surface("stalled"), now + Duration::from_secs(6));
    assert!(
        serde_json::to_string(&wire_messages(&b.take_output()))
            .unwrap()
            .contains("stalled")
    );
    b.stop(true);
    assert!(wire_messages(&b.take_output()).iter().any(|(v, _)| *v == 'x'));
    assert!(!b.active());
    b.expect_hello_for_test(now+Duration::from_secs(7));
    b.message(hello(2));
    b.paint(surface("reopened"), now + Duration::from_secs(7));
    assert!(wire_messages(&b.take_output()).iter().any(|(v, _)| *v == 'o'));
    let mut unsupported=Backend::default();unsupported.expect_hello_for_test(now);
    unsupported.message(tsp_wire::Message{verb:'r',value:json!({"r":"hello","v":999,"term":"bad-version","kinds":[]})});
    assert!(!unsupported.active());assert!(matches!(unsupported.paint(Surface::default(),now),Paint::Rows));
    b.revoke();assert!(!b.active());assert!(wire_messages(&b.take_output()).iter().any(|(v,_)|*v=='x'));
}
#[test]
fn target_original_graph_facts_growth_rewind_and_final_cache_bank(){
 let realm=crate::native_realm::NativeRealm::prepare().unwrap();let callbacks=realm.begin_display_callbacks().unwrap();let runtime=callbacks.runtime().unwrap();let g=super::graph_renderer::DisplayGraph(&realm.values,&runtime);let heap=realm.values.heap();
 let context=EditRenderContext{edit_mode:Some(ara_edit::EditMode::Hashline),..Default::default()};let partial=RenderOptions{is_partial:true,..Default::default()};
 let first="[a.rs#ABCD]\nPUT 1:\n+one\n";let second=format!("{first}[b.rs#ABCD]\nPUT 1:\n+two\n");let root=heap.import_serde(&json!({"input":first,"__partialJson":"raw-prefix"})).unwrap();let(_,facts)=super::graph_renderer::GraphEditRenderer::native_call_args(&g,&root,partial,&context).unwrap();assert_eq!(facts.file_count,1);
 let grown=heap.import_serde(&json!({"input":second,"__partialJson":"raw-prefix-continues"})).unwrap();let(_,gated)=super::graph_renderer::GraphEditRenderer::native_call_args(&g,&grown,partial,&context).unwrap();assert_eq!(gated.file_count,1,"fresh growing raw payload below512 retains the same source facts");let(_,final_facts)=super::graph_renderer::GraphEditRenderer::native_call_args(&g,&grown,RenderOptions::default(),&context).unwrap();assert_eq!(final_facts.file_count,2,"final args always recompute");
 let rewind=heap.import_serde(&json!({"input":"[rewound.rs#ABCD]\n","__partialJson":"other"})).unwrap();let(_,facts)=super::graph_renderer::GraphEditRenderer::native_call_args(&g,&rewind,partial,&context).unwrap();assert_eq!(facts.path,"rewound.rs");
 let sloppy=heap.import_serde(&json!({"input":"*** Edit File: src/a.rs\n*** Find\nold\n*** Replace\nnew"})).unwrap();let context=EditRenderContext{edit_mode:Some(ara_edit::EditMode::Sloppy),..Default::default()};super::graph_renderer::GraphEditRenderer::native_call_args(&g,&sloppy,RenderOptions::default(),&context).unwrap();let key=heap.module_symbol("terminal.edit","edit.inspectedInput").unwrap();let memo=heap.get(&sloppy,&key,&runtime).unwrap();let entries=heap.get(&memo,&"entries".into(),&runtime).unwrap();super::graph_renderer::GraphEditRenderer::native_call_args(&g,&sloppy,RenderOptions::default(),&context).unwrap();let after=heap.get(&heap.get(&sloppy,&key,&runtime).unwrap(),&"entries".into(),&runtime).unwrap();assert!(after.same_identity(&entries),"same original args retains the memoized native inspection array");
}
