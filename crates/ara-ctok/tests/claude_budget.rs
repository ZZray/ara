use ara_ctok::{
    ClaudeFamily, ContentBudget, Encoding, check_content_budget, count_content, count_encoding_fragments,
    count_fragments,
};

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

#[test]
fn encoding_fragments_preserve_content_boundaries_for_every_family() {
    let fragments = ["a", "b", "", "e\u{301}", "\n"];
    for encoding in [
        Encoding::O200kBase,
        Encoding::Cl100kBase,
        Encoding::ClaudeV3,
        Encoding::ClaudeV47,
        Encoding::ClaudeV5,
        Encoding::ClaudeV5Sonnet,
        Encoding::Qwen3,
        Encoding::DeepSeekV3,
        Encoding::KimiK2,
        Encoding::Glm5,
    ] {
        let expected: u64 = fragments.iter().map(|text| u64::from(encoding.count(*text))).sum();
        assert_eq!(count_encoding_fragments(fragments, encoding), Some(expected), "{encoding:?}");
        assert_eq!(count_encoding_fragments([], encoding), Some(0), "{encoding:?}");
    }

    // These pieces merge when joined. Fragment summation must preserve the
    // caller's content boundaries instead of retokenizing their concatenation.
    assert_eq!(count_encoding_fragments(["a", "b"], Encoding::Cl100kBase), Some(2));
    assert_eq!(Encoding::Cl100kBase.count("ab"), 1);

    for (family, encoding) in [
        (ClaudeFamily::V3, Encoding::ClaudeV3),
        (ClaudeFamily::V47, Encoding::ClaudeV47),
        (ClaudeFamily::V5, Encoding::ClaudeV5),
        (ClaudeFamily::V5Sonnet, Encoding::ClaudeV5Sonnet),
    ] {
        assert_eq!(count_encoding_fragments(fragments, encoding), count_fragments(fragments, family));
    }
}
