use ara_ctok::{ClaudeFamily, ContentBudget, check_content_budget, count_content, count_fragments};

#[test]
fn pinned_claude_examples_disprove_the_byte_shortcut() {
    assert_eq!(count_content("", ClaudeFamily::V3), 1);
    assert_eq!(count_content("", ClaudeFamily::V47), 1);
    assert_eq!(count_content("", ClaudeFamily::V5), 0);
    for family in [ClaudeFamily::V3, ClaudeFamily::V47, ClaudeFamily::V5] {
        assert_eq!(count_content("ξ", family), 3);
        assert_eq!(check_content_budget(["ξ"], family, 2), ContentBudget::Exceeds { tokens: 3 });
        assert_eq!(check_content_budget(["ξ"], family, 3), ContentBudget::Fits { tokens: 3 });
    }
}

#[test]
fn fragments_are_counted_separately() {
    let family = ClaudeFamily::V3;
    let expected = u64::from(count_content("a", family)) + u64::from(count_content("b", family));
    assert_eq!(count_fragments(["a", "b"], family), Some(expected));
    assert_eq!(check_content_budget(["a", "b"], family, expected - 1), ContentBudget::Exceeds { tokens: expected });
}
