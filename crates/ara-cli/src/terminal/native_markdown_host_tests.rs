//! Native Host connection supplements; no new fixed OMP original IDs.
use super::*;
use crate::terminal::types::FrameContext;

fn rendered(controller: &mut EventController, key: &BlockKey, commit: bool) -> Vec<String> {
    let mut scratch = controller.session_frame_copy(key).expect("renderer-backed SessionRow");
    let rows = scratch.render_for_frame(80, &FrameContext { true_color: true, ..Default::default() });
    if commit { controller.commit_session_row(key, scratch); }
    rows
}
fn observation(messages: Vec<(String, SessionWire)>) -> SessionTranscriptObservation {
    SessionTranscriptObservation {
        owner: None, session_id: "native-markdown-host".into(), revision: 1, collapsed: false,
        view: ara_session::context_view::TranscriptView {
            messages: messages.into_iter().map(|(entry_id, body)| ara_session::context_view::TranscriptViewMessage { entry_id, body }).collect(),
            cache_miss_explained_at: Vec::new(),
        }, originals: HashMap::new(),
    }
}
fn text_body(source: &str) -> SessionWire {
    SessionWire::object(vec![("role", SessionWire::String("assistant".into())),
        ("content", SessionWire::Array(vec![SessionWire::object(vec![("type", SessionWire::String("text".into())), ("text", SessionWire::String(source.into()))])]))])
}

#[test]
fn native_markdown_host_connection_progressive_final_restore_and_theme() {
    for (language, code, color) in [
        ("python", "value = \"\"\"open\ninside\nTAIL", "syntaxString"),
        ("javascript", "/* open\ninside\nTAIL", "syntaxComment"),
    ] {
        let theme = Arc::new(Theme::dark());
        let mut controller = EventController::new(theme.clone(), ara_edit::EditMode::Hashline);
        controller.begin_run(ara_edit::EditMode::Hashline);
        controller.start_entry_observation();
        let source = format!("```{language}\n{code}");
        controller.set_stream_text(source.clone());
        let key = BlockKey::Text(controller.text.unwrap());
        assert_eq!(controller.transcript.active_keys(), vec![key.clone()]);
        let highlighter = controller.native_markdown_options.highlighter.clone().unwrap();
        let factory = controller.native_markdown_options.highlight_stream_factory.clone().unwrap();
        let active = rendered(&mut controller, &key, true);
        assert!(active.iter().any(|row| row.contains("inside") && row.contains(theme.ansi(color))), "{active:?}");
        assert!(active.iter().any(|row| row.contains("TAIL") && row.contains(theme.ansi("mdCodeBlock"))), "{active:?}");
        assert!(Arc::ptr_eq(&highlighter, controller.native_markdown_options.highlighter.as_ref().unwrap()));
        assert!(Arc::ptr_eq(&factory, controller.native_markdown_options.highlight_stream_factory.as_ref().unwrap()));
        // Rejected scratch must not advance retained native multiline parser.
        let rejected = rendered(&mut controller, &key, false);
        assert_eq!(rejected, rendered(&mut controller, &key, false));
        let extended = format!("{source}\nsecond\nPART");
        controller.set_stream_text(extended.clone());
        let scratch = rendered(&mut controller, &key, false);
        assert_eq!(scratch, rendered(&mut controller, &key, true));
        let closing = if language == "python" { "\"\"\"" } else { "*/" };
        let final_source = format!("{extended}\n{closing}\n```\n");
        controller.set_stream_text(final_source.clone());
        controller.finish_text();
        assert!(controller.transcript.active_keys().is_empty());
        let final_rows = rendered(&mut controller, &key, true);
        let mut whole = host_markdown(final_source.clone().into(), theme.clone(), controller.native_markdown_options.clone());
        whole.set_frame_capabilities(&FrameContext { true_color: true, ..Default::default() });
        assert_eq!(final_rows, *whole.render(80));
        controller.observe_entry_receipt("live-receipt".into(), Arc::new(text_body(&final_source))).unwrap();
        assert_eq!(controller.session_transcript.entries["live-receipt"].keys, vec![key.clone()]);
        // Restored rows bind the same native adapter; no new tool identities.
        let mut restored = EventController::new(theme.clone(), ara_edit::EditMode::Hashline);
        restored.observe_session(observation(vec![("restore".into(), text_body(&final_source))]), None).unwrap();
        assert_eq!(rendered(&mut restored, &BlockKey::Session("restore".into(), 1), true), final_rows);
        assert!(restored.transcript.tool_keys().is_empty());
        // Existing invalidation rebinds both callbacks on a real theme change.
        controller.theme = Arc::new(Theme::light());
        controller.invalidate_session_rows();
        assert!(!Arc::ptr_eq(&highlighter, controller.native_markdown_options.highlighter.as_ref().unwrap()));
        assert!(!Arc::ptr_eq(&factory, controller.native_markdown_options.highlight_stream_factory.as_ref().unwrap()));
        let changed = rendered(&mut controller, &key, true);
        assert!(changed.iter().any(|row| row.contains("inside") && row.contains(controller.theme.ansi(color))), "{changed:?}");
        let rebound = controller.native_markdown_options.highlighter.clone().unwrap();
        controller.invalidate_session_rows();
        assert!(Arc::ptr_eq(&rebound, controller.native_markdown_options.highlighter.as_ref().unwrap()));
        // Plain generic tool output never acquires a Markdown SessionRow.
        controller.result("generic".into(), "bash".into(), EditResult { content: vec!["```python\ninside\n```".into()], ..Default::default() }, false);
        let generic = controller.transcript.block_keys().last().unwrap().clone();
        assert!(controller.session_frame_copy(&generic).is_none());
    }
    // Summary detail is reconstructed after theme/options invalidation.
    let summary = SessionWire::object(vec![("role", SessionWire::String("compactionSummary".into())),
        ("summary", SessionWire::String("```python\nvalue = \"inside\"\n```".into()))]);
    let mut controller = EventController::new(Arc::new(Theme::dark()), ara_edit::EditMode::Hashline);
    controller.observe_session(observation(vec![("summary".into(), summary)]), None).unwrap();
    let key = BlockKey::Session("summary".into(), 0);
    if let SessionRow::Summary(component) = controller.session_transcript.rows.get_mut(&key).unwrap() { component.set_expanded(true); } else { panic!("summary owner"); }
    assert!(rendered(&mut controller, &key, true).iter().any(|row| row.contains("inside") && row.contains(controller.theme.ansi("syntaxString"))));
    controller.theme = Arc::new(Theme::light());
    controller.invalidate_session_rows();
    assert!(rendered(&mut controller, &key, true).iter().any(|row| row.contains("inside") && row.contains(controller.theme.ansi("syntaxString"))));
}
