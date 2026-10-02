use ara_agent::tokenizer::{
    EstimateMode, IMAGE_TOKEN_ESTIMATE, MessageCountOptions, ModelContentCount, SNAPCOMPACT_FRAME_TOKEN_ESTIMATE,
    count_fragments, count_message, count_messages, count_model_fragments, count_text,
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
