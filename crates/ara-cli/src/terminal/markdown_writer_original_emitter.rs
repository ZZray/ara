//! Auxiliary process emitter; original B-deacfbb9e3 is accepted only by the cell oracle.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
#[test]
#[ignore = "auxiliary terminal emitter; run explicitly under 80x6 ConPTY"]
fn emit_writer1_vt() {
    let out = PathBuf::from(std::env::var_os("ARA_WRITER1_EMITTER_OUT").expect("fresh oracle output directory"));
    let (theme, mut options) = capture_tests::fixed_default_markdown_theme();
    options.padding_x = 1;
    options.padding_y = 0;
    options.default_style.color = Some(Arc::new(|text| {
        if text.is_empty() {
            String::new()
        } else {
            format!("\x1b[90m{}\x1b[39m", text.replace("\x1b[39m", "\x1b[39m\x1b[90m"))
        }
    }));
    options.default_style.italic = true;
    let mut md = MarkdownComponent::new(WireString::from("This is thinking with `inline code`"), theme);
    md.set_options(options);
    let mut rows = md.render(80).as_ref().clone();
    let count = rows.len();
    assert!(count > 0 && count < 6);
    std::fs::write(out.join("markdown-rows.json"), serde_json::to_vec(&rows).unwrap()).unwrap();
    rows.push("INPUT".to_owned());
    super::super::driver::emit_original_writer_rows(rows).unwrap();
    let meta = serde_json::json!({"originalID":"B-deacfbb9e3", "markdownLineCount":count,
        "columns":80,"rows":6,"writer":"paint->compose_frame->safe_write",
        "acceptance":"AWAITING_RAW_CONPTY_CELL_ORACLE"});
    std::fs::write(out.join("frame-ready.json"), serde_json::to_vec_pretty(&meta).unwrap()).unwrap();
    // Preserve this exact screen before libtest prints its suffix or teardown.
    let start = Instant::now();
    while !out.join("release-emitter").exists() {
        assert!(start.elapsed() < Duration::from_secs(60), "oracle failed to release emitter");
        std::thread::sleep(Duration::from_millis(10));
    }
}
