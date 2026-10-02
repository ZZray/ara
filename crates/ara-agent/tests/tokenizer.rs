use ara_agent::tokenizer::{
    EstimateMode, IMAGE_TOKEN_ESTIMATE, MessageCountOptions, ModelContentCount, SNAPCOMPACT_FRAME_TOKEN_ESTIMATE,
    TokenCountMode, Tokenizer, TokenizerPolicy, count_fragments, count_message, count_messages, count_model_fragments,
    count_text,
};
use ara_ai::{
    AssistantBlock, AssistantMessage, DeveloperMessage, ImageContent, Message, Model, ModelTokenizer, TextContent,
    ThinkingContent, ToolCall, ToolResultMessage, UserBlock, UserContent, UserMessage,
};
use serde_json::{Map, Value};

fn image() -> ImageContent {
    ImageContent { detail: None, compaction_frame: false, data: "AA==".into(), mime_type: "image/png".into() }
}

fn model(tokenizer: Option<ModelTokenizer>) -> Model {
    Model {
        id: "alias".into(),
        api: "openai-completions".into(),
        provider: "test".into(),
        base_url: "http://localhost".into(),
        reasoning: false,
        max_tokens: None,
        context_window: None,
        tokenizer,
    }
}

#[test]
fn explicit_model_family_selects_exact_content_counter() {
    assert_eq!(count_model_fragments(&model(None), ["ξ"]), ModelContentCount::UnknownTokenizer);
    for family in
        [ModelTokenizer::ClaudeV3, ModelTokenizer::ClaudeV47, ModelTokenizer::ClaudeV5, ModelTokenizer::ClaudeV5Sonnet]
    {
        assert_eq!(count_model_fragments(&model(Some(family)), ["ξ"]), ModelContentCount::Exact(3));
    }
    let mut selected = model(Some(ModelTokenizer::ClaudeV3));
    assert_eq!(count_model_fragments(&selected, ["hello, world"]), ModelContentCount::Exact(3));
    selected.tokenizer = Some(ModelTokenizer::ClaudeV47);
    assert_eq!(count_model_fragments(&selected, ["hello, world"]), ModelContentCount::Exact(4));
    assert_eq!(count_model_fragments(&selected, ["", "ξ"]), ModelContentCount::Exact(4));
    selected.tokenizer = Some(ModelTokenizer::ClaudeV5);
    assert_eq!(count_model_fragments(&selected, ["a\n "]), ModelContentCount::Exact(1));
    selected.tokenizer = Some(ModelTokenizer::ClaudeV5Sonnet);
    assert_eq!(count_model_fragments(&selected, ["a\n "]), ModelContentCount::Exact(3));
}

#[test]
fn utf8_estimates_round_each_fragment() {
    assert_eq!(count_text("hello world", EstimateMode::Approximate), 3);
    assert_eq!(count_text("hello world", EstimateMode::RawUtf8Bytes), 11);
    assert_eq!(count_fragments(["é", "a"], EstimateMode::Approximate), 2);
    assert_eq!(count_fragments(["éa"], EstimateMode::Approximate), 1);
    assert_eq!(count_fragments(["é", "a"], EstimateMode::RawUtf8Bytes), 3);
}

#[test]
fn raw_bytes_are_only_an_observation() {
    assert_eq!(count_fragments([""], EstimateMode::RawUtf8Bytes), 0);
    assert_eq!(count_fragments(["ξ"], EstimateMode::RawUtf8Bytes), 2);
    // The pinned Claude fixture counts this two-byte text as three content
    // tokens, so no fit/exceeds verdict may be inferred from this value.
}

#[test]
fn estimates_supported_message_blocks_without_stale_cache() {
    let mut user = Message::User(UserMessage::text("a"));
    assert_eq!(count_message(&user, MessageCountOptions::default()), 1);
    if let Message::User(message) = &mut user {
        message.content = UserContent::Blocks(vec![UserBlock::text("abcde"), UserBlock::Image(image())]);
    }
    // The fixed OMP implementation estimates user text but not user images.
    assert_eq!(count_message(&user, MessageCountOptions::default()), 2);

    if let Message::User(message) = &mut user {
        let mut frame = image();
        frame.compaction_frame = true;
        frame.detail = Some(serde_json::json!("original"));
        message.content = UserContent::Blocks(vec![
            UserBlock::text("abcde"),
            UserBlock::Image(image()),
            UserBlock::Image(frame.clone()),
            UserBlock::Image(frame),
        ]);
    }
    assert_eq!(
        count_message(&user, MessageCountOptions::default()),
        2 + 2 * SNAPCOMPACT_FRAME_TOKEN_ESTIMATE,
        "derived archive frames count 5024 each; original user pictures remain zero"
    );

    let developer = Message::Developer(DeveloperMessage { content: UserContent::Text("测试".into()), timestamp: 0 });
    assert_eq!(count_message(&developer, MessageCountOptions::default()), 2);

    let mut arguments = Map::new();
    arguments.insert("path".into(), Value::String("a".into()));
    arguments.insert("meta".into(), serde_json::json!({"lang": "中文"}));
    let argument_tokens = count_text(&serde_json::to_string(&arguments).unwrap(), EstimateMode::Approximate);
    let mut assistant = AssistantMessage::empty("openai-completions", "test", "test");
    assistant.content = vec![
        AssistantBlock::Text(TextContent { text: "hi".into(), text_signature: None }),
        AssistantBlock::Thinking(ThinkingContent {
            thinking: "think".into(),
            thinking_signature: Some("abcdefgh".into()),
        }),
        AssistantBlock::RedactedThinking { data: "opaque".into() },
        AssistantBlock::ToolCall(ToolCall {
            id: "call-1".into(),
            name: "read".into(),
            arguments,
            thought_signature: None,
        }),
        AssistantBlock::Image(image()),
    ];
    let assistant = Message::Assistant(assistant);
    let full = count_message(&assistant, MessageCountOptions::default());
    let floored = count_message(&assistant, MessageCountOptions { exclude_encrypted_reasoning: true });
    let visible = count_fragments(["hi", "think", "read"], EstimateMode::Approximate);
    assert_eq!(floored, visible + argument_tokens + IMAGE_TOKEN_ESTIMATE);
    assert_eq!(full, floored + count_fragments(["abcdefgh", "opaque"], EstimateMode::Approximate));

    let tool_result = Message::ToolResult(ToolResultMessage {
        tool_call_id: "call-1".into(),
        tool_name: "read".into(),
        content: vec![UserBlock::text("hello"), UserBlock::Image(image())],
        details: None,
        is_error: false,
        timestamp: 0,
    });
    assert_eq!(count_message(&tool_result, MessageCountOptions::default()), 2 + IMAGE_TOKEN_ESTIMATE);

    let messages = [user, developer, assistant, tool_result];
    assert_eq!(
        count_messages(&messages, MessageCountOptions::default()),
        messages.iter().map(|message| count_message(message, MessageCountOptions::default())).sum::<usize>()
    );
}

fn exact(family: ModelTokenizer, fragments: &[&str]) -> usize {
    match count_model_fragments(&model(Some(family)), fragments.iter().copied()) {
        ModelContentCount::Exact(tokens) => usize::try_from(tokens).unwrap(),
        other => panic!("expected exact {family:?} content count, got {other:?}"),
    }
}

#[test]
fn every_family_and_mode_use_the_captured_catalog_choice() {
    let fragments = ["hello, world", "ξ", "你好", ""];
    for family in [
        ModelTokenizer::ClaudeV3,
        ModelTokenizer::ClaudeV47,
        ModelTokenizer::ClaudeV5,
        ModelTokenizer::ClaudeV5Sonnet,
        ModelTokenizer::Qwen3,
        ModelTokenizer::DeepSeekV3,
        ModelTokenizer::KimiK2,
        ModelTokenizer::Glm5,
    ] {
        let tokenizer = Tokenizer::for_model(&model(Some(family)));
        assert_eq!(tokenizer.encoding(), Some(family));
        for mode in [TokenCountMode::Strict, TokenCountMode::Approximate, TokenCountMode::UpperBound] {
            assert_eq!(tokenizer.count_fragments(fragments, mode), exact(family, &fragments), "{family:?} {mode:?}");
        }
    }

    let mut selected = model(Some(ModelTokenizer::ClaudeV3));
    let captured = Tokenizer::for_model(&selected);
    selected.tokenizer = Some(ModelTokenizer::ClaudeV47);
    let changed = Tokenizer::for_model(&selected);
    assert_eq!(captured.count_text("hello, world", TokenCountMode::Strict), 3);
    assert_eq!(changed.count_text("hello, world", TokenCountMode::Strict), 4);
    assert_eq!(captured.encoding(), Some(ModelTokenizer::ClaudeV3));
}

#[test]
fn unknown_accurate_and_test_policies_keep_strict_native() {
    let unknown = Tokenizer::with_policy(None, TokenizerPolicy::default());
    assert_eq!(unknown.count_text("hello world", TokenCountMode::Approximate), 3);
    assert_eq!(unknown.count_text("hello world", TokenCountMode::UpperBound), 11);
    assert_eq!(unknown.count_text("hello world", TokenCountMode::Strict), 2);
    // Fragment boundaries survive even when the default encoding could merge them.
    assert_eq!(unknown.count_fragments(["a", "b"], TokenCountMode::Strict), 2);
    assert_eq!(unknown.count_text("ab", TokenCountMode::Strict), 1);

    let accurate = Tokenizer::with_policy(None, TokenizerPolicy { test_environment: false, accurate_unknown: true });
    assert_eq!(accurate.count_text("hello world", TokenCountMode::Approximate), 2);
    assert_eq!(accurate.count_text("hello world", TokenCountMode::UpperBound), 2);
    for selected in [model(None), model(Some(ModelTokenizer::ClaudeV47))] {
        let test =
            Tokenizer::with_policy(Some(&selected), TokenizerPolicy { test_environment: true, accurate_unknown: true });
        assert_eq!(test.count_text("hello world", TokenCountMode::Approximate), 3);
        assert_eq!(test.count_text("hello world", TokenCountMode::UpperBound), 11);
        let strict = selected.tokenizer.map_or(2, |family| exact(family, &["hello world"]));
        assert_eq!(test.count_text("hello world", TokenCountMode::Strict), strict);
    }
}

#[test]
fn budget_verdict_measures_native_even_when_bytes_fit_or_policy_is_test() {
    let tokenizer = Tokenizer::with_policy(
        Some(&model(Some(ModelTokenizer::ClaudeV3))),
        TokenizerPolicy { test_environment: true, accurate_unknown: false },
    );
    let exceeds = tokenizer.check_token_budget(["ξ"], 2);
    assert!(!exceeds.fits);
    assert_eq!(exceeds.tokens, 3);
    assert!(exceeds.exact);
    assert!(tokenizer.check_token_budget(["ξ"], 3).fits);
    assert_eq!(tokenizer.check_token_budget([""], 0).tokens, 1);
    assert!(!tokenizer.check_token_budget([""], 0).fits);
    assert!(tokenizer.check_token_budget([], 0).fits);
    let unknown = Tokenizer::with_policy(None, TokenizerPolicy::default());
    assert_eq!(unknown.check_token_budget(["hello world"], 2).tokens, 2);
    assert!(unknown.check_token_budget(["hello world"], 2).fits);
}

#[test]
fn typed_empty_fragments_reasoning_tools_and_images_follow_fixed_rules() {
    let tokenizer = Tokenizer::for_model(&model(Some(ModelTokenizer::ClaudeV3)));
    let options = MessageCountOptions::default();
    let empty_string = Message::User(UserMessage::text(""));
    let empty_block = Message::User(UserMessage {
        content: UserContent::Blocks(vec![UserBlock::text(""), UserBlock::Image(image())]),
        synthetic: None,
        timestamp: 0,
    });
    assert_eq!(tokenizer.count_message(&empty_string, options), 1);
    assert_eq!(tokenizer.count_message(&empty_block, options), 0);
    assert_eq!(
        tokenizer.count_message(
            &Message::Developer(DeveloperMessage { content: UserContent::Text("".into()), timestamp: 0 }),
            options
        ),
        1
    );

    let mut assistant = AssistantMessage::empty("anthropic-messages", "test", "alias");
    assistant.content = vec![
        AssistantBlock::Text(TextContent { text: "".into(), text_signature: None }),
        AssistantBlock::Thinking(ThinkingContent { thinking: "ξ".into(), thinking_signature: Some("ξ".into()) }),
        AssistantBlock::Thinking(ThinkingContent { thinking: "".into(), thinking_signature: Some("".into()) }),
        AssistantBlock::RedactedThinking { data: "".into() },
        AssistantBlock::ToolCall(ToolCall {
            id: "a".into(),
            name: "ξ".into(),
            arguments: Map::new(),
            thought_signature: None,
        }),
        AssistantBlock::Image(image()),
    ];
    let assistant = Message::Assistant(assistant);
    let expected = exact(ModelTokenizer::ClaudeV3, &["", "ξ", "ξ", "", "", "ξ", "{}"]);
    assert_eq!(tokenizer.count_message(&assistant, options), expected + IMAGE_TOKEN_ESTIMATE);
    assert_eq!(
        tokenizer.count_message(&assistant, MessageCountOptions { exclude_encrypted_reasoning: true }),
        exact(ModelTokenizer::ClaudeV3, &["", "ξ", "", "ξ", "{}"]) + IMAGE_TOKEN_ESTIMATE,
    );

    let tool = Message::ToolResult(ToolResultMessage {
        tool_call_id: "a".into(),
        tool_name: "ξ".into(),
        content: vec![UserBlock::text(""), UserBlock::text("ξ"), UserBlock::Image(image())],
        details: None,
        is_error: false,
        timestamp: 0,
    });
    assert_eq!(tokenizer.count_message(&tool, options), 3 + IMAGE_TOKEN_ESTIMATE);
    let mut frame = image();
    frame.compaction_frame = true;
    let archived = Message::User(UserMessage {
        content: UserContent::Blocks(vec![UserBlock::text("ξ"), UserBlock::Image(frame)]),
        synthetic: None,
        timestamp: 0,
    });
    assert_eq!(tokenizer.count_message(&archived, options), 3 + SNAPCOMPACT_FRAME_TOKEN_ESTIMATE);
    let messages = [empty_string, assistant, tool, archived];
    assert_eq!(
        tokenizer.count_messages(&messages, options),
        1 + expected + 2 * IMAGE_TOKEN_ESTIMATE + 6 + SNAPCOMPACT_FRAME_TOKEN_ESTIMATE
    );
}

#[test]
fn fresh_native_counts_follow_mutated_values_and_independent_clones() {
    let tokenizer = Tokenizer::for_model(&model(Some(ModelTokenizer::ClaudeV3)));
    let mut original = Message::User(UserMessage::text("ξ"));
    let clone = original.clone();
    assert_eq!(tokenizer.count_message(&original, Default::default()), 3);
    if let Message::User(user) = &mut original {
        user.content = UserContent::Text("".into());
    }
    assert_eq!(tokenizer.count_message(&original, Default::default()), 1);
    assert_eq!(tokenizer.count_message(&clone, Default::default()), 3);
}
