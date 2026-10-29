//! Fixed box-border.test.ts original IDs; native owned child lines.
use super::*;
struct Text {
    source: String,
}
impl StaticComponent for Text {
    fn render(&mut self, width: usize) -> Arc<Vec<String>> {
        Arc::new(render_utils::wrap(&self.source, width))
    }
    fn invalidate(&mut self, _: Arc<Theme>) {}
}
fn boxed() -> BoxComponent {
    let theme = Arc::new(Theme::from_json(r#"{"colors":{"testBorder":1,"testBg":236}}"#, "ascii").unwrap());
    let mut b = BoxComponent::new(
        1,
        0,
        Some("testBg".into()),
        Some(BoxBorder {
            top_left: "+".into(),
            top_right: "+".into(),
            bottom_left: "+".into(),
            bottom_right: "+".into(),
            horizontal: "-".into(),
            vertical: "|".into(),
            color: Some("testBorder".into()),
        }),
        theme,
    );
    b.set_bg_fn(Some(Arc::new(|s| format!("\x1b[48;5;236m{s}\x1b[49m"))));
    b.set_border_colorizer(Some(Arc::new(|s| format!("\x1b[31m{s}\x1b[39m"))));
    b.set_ignore_tight(true);
    b.add_child(Box::new(Text { source: "hi".into() }));
    b
}
fn plain(s: &str) -> String {
    regex::Regex::new(r"\x1b\[[0-9;:]*m").unwrap().replace_all(s, "").into_owned()
}
#[test]
fn original_box_border_five_ids() {
    let rows = boxed().render(20);
    assert_eq!(rows.len(), 3, "B-a069610c9b");
    for row in rows.iter() {
        assert_eq!(render_utils::width(row), 20, "B-a069610c9b width");
    }
    assert_eq!(plain(&rows[0]), format!("+{}+", "-".repeat(18)), "B-a069610c9b top");
    assert_eq!(plain(&rows[2]), format!("+{}+", "-".repeat(18)), "B-a069610c9b bottom");
    let row = plain(&rows[1]);
    assert!(row.starts_with('|') && row.ends_with('|') && row.contains("hi"), "B-a069610c9b content");
    assert!(rows[0].contains("\x1b[31m"), "B-254c6b3f89 supplied colorizer red border");
    let mut b = boxed();
    assert_eq!(
        b.render(20).iter().map(|s| render_utils::width(s)).collect::<Vec<_>>(),
        vec![20, 20, 20],
        "B-eb017ca7a6 initial"
    );
    b.set_border(None);
    let rows = b.render(20);
    assert_eq!(rows.len(), 1, "B-eb017ca7a6");
    assert_eq!(render_utils::width(&rows[0]), 20, "B-eb017ca7a6 width");
    assert!(!plain(&rows[0]).contains('+'), "B-eb017ca7a6 border removed");
    let rows = boxed().render(5);
    for row in rows.iter() {
        assert_eq!(render_utils::width(row), 5, "B-d839de7120");
    }
    assert_eq!(plain(&rows[0]), "+---+", "B-d839de7120 top");
    for width in [3, 4] {
        for row in boxed().render(width).iter() {
            assert!(render_utils::width(row) <= width, "B-b925d5d0fe width");
            assert!(!plain(row).contains('+'), "B-b925d5d0fe no border");
        }
    }
}
