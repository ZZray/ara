//! Fixed 596f2da compaction divider and handoff original assertions.
use super::*;
fn component(value: serde_json::Value) -> SummaryMessageComponent {
    let wire = WireValue::parse(&value.to_string()).unwrap();
    SummaryMessageComponent::from_wire(&wire, Arc::new(Theme::dark())).unwrap().unwrap()
}
fn compaction(extra: serde_json::Value) -> SummaryMessageComponent {
    let mut value = serde_json::json!({"role":"compactionSummary","summary":"Earlier the user fixed the login TTL bug.","tokensBefore":84000});
    value.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    component(value)
}
fn plain(s: &str) -> String {
    regex::Regex::new(r"\x1b\[[0-9;:]*m").unwrap().replace_all(s, "").into_owned()
}
#[test]
fn original_compaction_and_handoff_ten_ids() {
    let mut c = compaction(serde_json::json!({}));
    let lines = c.render(80);
    assert_eq!(lines.len(), 3, "B-9aa97b8ced rows");
    let rule = plain(&lines[1]);
    assert!(rule.contains("compacted") && rule.contains("ctrl+o"), "B-9aa97b8ced labels");
    assert_eq!(render_utils::width(&rule), 80, "B-9aa97b8ced width");
    assert!(!rule.contains("Earlier the user fixed the login TTL bug."), "B-9aa97b8ced collapsed");
    let mut c = compaction(serde_json::json!({"method":"remote","tokensBefore":256000,"tokensAfter":20000}));
    let rule = plain(&c.render(80)[1]);
    assert!(rule.contains("remote-compacted") && rule.contains("256K→20K") && rule.contains("ctrl+o"), "B-47c6306028");
    let mut c = compaction(serde_json::json!({"method":"handoff"}));
    let rule = plain(&c.render(80)[1]);
    assert!(rule.contains("handed-off") && !rule.contains('→'), "B-28302353de");
    let mut c = compaction(serde_json::json!({"method":"handoff","tokensBefore":0,"tokensAfter":48573}));
    c.set_expanded(true);
    let text = plain(&c.render(80).join("\n"));
    assert!(text.contains("Compacted to 48,573 tokens") && !text.contains("Compacted from 0"), "B-8c80491061");
    let mut c = compaction(serde_json::json!({"images":[{"type":"image","data":"ZmFrZQ==","mimeType":"image/png"}]}));
    c.set_expanded(true);
    let text = plain(&c.render(80).join("\n"));
    for part in ["compacted", "Earlier the user fixed the login TTL bug.", "tokens", "1 snapcompact frame attached"] {
        assert!(text.contains(part), "B-567e2fcf13 {part}");
    }
    assert!(plain(&compaction(serde_json::json!({})).render(10)[1]).contains("compacted"), "B-89a574dc81");
    let mut c = compaction(serde_json::json!({}));
    let first = c.render(80);
    assert!(Arc::ptr_eq(&first, &c.render(80)), "B-3476886094 L1");
    c.set_expanded(true);
    let expanded = c.render(80);
    assert!(!Arc::ptr_eq(&first, &expanded), "B-3476886094 bust");
    assert!(Arc::ptr_eq(&expanded, &c.render(80)), "B-3476886094 expanded L1");
    let mut c = component(
        serde_json::json!({"role":"custom","customType":"handoff","display":true,"content":"<handoff-context>\n# Goal\nContinue the resize fix.\n</handoff-context>\n\nThe above is a handoff document."}),
    );
    let collapsed = plain(&c.render(80).join("\n"));
    assert!(
        collapsed.contains("handed-off")
            && collapsed.contains("ctrl+o")
            && !collapsed.contains("[handoff]")
            && !collapsed.contains("Continue the resize fix"),
        "B-9433e7e52e"
    );
    let mut c = component(
        serde_json::json!({"role":"custom","customType":"handoff","display":true,"content":[{"type":"text","text":"<handoff-context>\n# Goal\nContinue the resize fix.\n</handoff-context>"}]}),
    );
    c.set_expanded(true);
    let expanded = plain(&c.render(80).join("\n"));
    assert!(
        expanded.contains("Handoff context")
            && expanded.contains("Continue the resize fix")
            && !expanded.contains("<handoff-context>")
            && !expanded.contains("</handoff-context>"),
        "B-c69be8a58c"
    );
    let wire = WireValue::parse(
        r#"{"role":"custom","customType":"extension-note","display":true,"content":"Not a handoff."}"#,
    )
    .unwrap();
    assert!(SummaryMessageComponent::from_wire(&wire, Arc::new(Theme::dark())).unwrap().is_none(), "B-3df2f5ef9e");
    observe_frame_options_and_owned_source();
}

// Supplemental Host/cache seam assertions; no additional original ID.
fn observe_frame_options_and_owned_source() {
    use super::super::{markdown::DefaultTextStyle, types::TerminalImageProtocol};
    let source = WireValue::parse(r##"{"role":"branchSummary","summary":"# Heading\n\n[link](https://example.com)\n\n\ud800","extra":{"retained":true}}"##).unwrap();
    let color: super::super::markdown::StyleFunction = Arc::new(|s| format!("<C>{s}</C>"));
    let mut c = SummaryMessageComponent::from_wire_with_options(
        &source,
        Arc::new(Theme::dark()),
        MarkdownOptions {
            default_style: DefaultTextStyle { color: Some(color.clone()), ..Default::default() },
            ..Default::default()
        },
    )
    .unwrap()
    .unwrap();
    c.set_expanded(true);
    let frame = FrameContext { columns: 80, hyperlinks: false, ..Default::default() };
    assert!(c.set_frame_capabilities(&frame));
    let first = c.render(80);
    assert!(!first.join("\n").contains("\x1b]8;"));
    assert!(!first.join("\n").contains("\x1b]66;"));
    assert!(!c.set_frame_capabilities(&frame));
    assert!(Arc::ptr_eq(&first, &c.render(80)));
    let mut scratch = c.frame_copy();
    let updated = FrameContext {
        columns: 80,
        hyperlinks: true,
        true_color: true,
        text_sizing: true,
        image_protocol: Some(TerminalImageProtocol::Kitty),
        ..Default::default()
    };
    assert!(scratch.set_frame_capabilities(&updated));
    let changed = scratch.render(80);
    assert!(changed.join("\n").contains("\x1b]8;"));
    assert!(changed.join("\n").contains("\x1b]66;"));
    assert!(Arc::ptr_eq(&first, &c.render(80)), "scratch capabilities do not mutate retained cache");
    assert!(c.source().deep_equal(&source));
    assert!(scratch.source().deep_equal(&source));
    assert!(scratch.source().get("summary").unwrap().as_string().unwrap().units().contains(&0xd800));
    assert!(Arc::ptr_eq(scratch.markdown_options.default_style.color.as_ref().unwrap(), &color));
    for protocol in [TerminalImageProtocol::Kitty, TerminalImageProtocol::Iterm2, TerminalImageProtocol::Sixel] {
        scratch.set_frame_capabilities(&FrameContext { image_protocol: Some(protocol), ..updated });
        assert_eq!(scratch.markdown_options.image_protocol.as_deref(), Some(protocol.as_sequence_prefix()));
    }
}
