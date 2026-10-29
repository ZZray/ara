//! Fixed b91db66e image-tokenization and snapcompact shape/budget behavior.
//! Upstream fixture values are historical provider measurements, not a fresh model call.
use ara_snapcompact::*;
use serde_json::{Value, json};

fn target(provider: &str, api: &str, id: &str) -> ShapeTarget {
    ShapeTarget { provider: Some(provider.into()), api: Some(api.into()), id: Some(id.into()), ..Default::default() }
}
fn size(width: f64, height: f64) -> ImageSize {
    ImageSize { width, height }
}
fn billing(provider: &str, api: &str, id: &str) -> FrameBilling {
    frame_billing(Some(&target(provider, api, id))).unwrap()
}
fn deployment(provider: &str, api: &str, id: &str, official: bool) -> DeploymentTarget {
    DeploymentTarget { model: target(provider, api, id), official_endpoint: official, ..Default::default() }
}

#[test]
fn catalog_lineage_wire_fallback_built_identity_and_provider_overrides() {
    // Strict model-policy classification keeps unresolved family ambiguity;
    // lenient image lookup can still price the class's conservative lineage.
    let ambiguous = deployment("anthropic", "anthropic-messages", "claude-sonnet-mythos", true);
    assert!(reader_images_read_results(&ambiguous).unwrap_err().to_string().contains("ambiguous family"));
    assert_eq!(billing("anthropic", "anthropic-messages", "claude-sonnet-mythos")
        .rule.unwrap()["maxTokens"], 4784);
    let namespaced = deployment("litellm", "openai-completions", "mistral.mixtral-8x7b-instruct-v0:1", false);
    assert_eq!(provider_image_budget(Some(&namespaced)).unwrap(), 5);
    // The 21 authored image-rule payloads are carried intact, rather than a hand picked model table.
    let rules: Value = serde_json::from_str(include_str!("../src/catalog-rules.json")).unwrap();
    let payloads: Vec<_> = rules["cascade"]["rules"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|rule| rule["catalog"].get("imageTokenization"))
        .collect();
    assert_eq!(payloads.len(), 21);
    for rule in payloads {
        let bill = FrameBilling { rule: Some(rule.clone()), image_detail: Some("original".into()) };
        let price = frame_tokens(&bill, size(1932.0, 1920.0));
        assert!(price.is_finite() && price > 0.0, "unpriced compiled payload {rule}");
    }
    for id in ["claude-opus-4-6", "claude-haiku-4-5", "claude-3-5-sonnet-20241022", "claude-sonnet-45"] {
        let b = billing("anthropic", "anthropic-messages", id);
        assert_eq!(b.rule.as_ref().unwrap()["maxTokens"], 1568, "{id}");
        assert_eq!(frame_tokens(&b, size(1568.0, 1568.0)), 1521.0);
        assert_ne!(b.rule.unwrap().get("verified"), Some(&json!(true)));
    }
    for id in ["claude-sonnet-4-6", "claude-sonnet-46"] {
        assert_eq!(billing("anthropic", "anthropic-messages", id).rule.unwrap()["verified"], true);
    }
    for id in [
        "claude-opus-4-7",
        "claude-opus-5-5",
        "claude-sonnet-5-5",
        "claude-fable-5-1",
        "claude-fable-latest",
        "claude-mythos-preview",
        "claude-opus-47",
    ] {
        let b = billing("anthropic", "anthropic-messages", id);
        assert_eq!(b.rule.as_ref().unwrap()["maxTokens"], 4784, "{id}");
        assert_eq!(b.rule.as_ref().unwrap()["verified"], true);
        assert_eq!(frame_tokens(&b, size(1932.0, 1920.0)), 4761.0);
    }
    assert_eq!(
        billing("openrouter", "openai-completions", "anthropic/claude-opus-4.7").rule,
        billing("anthropic", "anthropic-messages", "claude-opus-4.7").rule
    );
    assert_eq!(
        billing("amazon-bedrock", "bedrock-converse-stream", "us.anthropic.claude-opus-4-7-v1:0").rule,
        billing("anthropic", "anthropic-messages", "claude-opus-4.7").rule
    );
    for id in ["gpt-6-astra", "gpt-6.1-sol", "gpt-5.6-luna", "gpt-5.5", "gpt-daybreak-blue-latest"] {
        let b = billing("openai-codex", "openai-codex-responses", id);
        assert_eq!(b.rule.as_ref().unwrap()["regime"], "openai-patch", "{id}");
        assert_eq!(b.rule.as_ref().unwrap()["verified"], true);
        assert_eq!(frame_tokens(&b, size(1568.0, 1562.0)), 2882.0);
    }
    for id in ["gpt-5.4", "gpt-5.3-codex", "gpt-6.2-sol", "gpt-7-luna"] {
        assert!(billing("openai", "openai-responses", id).rule.unwrap().get("verified").is_none(), "{id}");
    }
    for id in ["gemini-2.5-flash-lite", "gemini-3.1-flash-lite", "gemini-3.8-flash"] {
        let b = billing("google-antigravity", "google-gemini-cli", id);
        assert_eq!(b.rule.as_ref().unwrap(), &json!({"regime":"fixed", "verified":true, "tokens":1089}));
        assert_eq!(frame_tokens(&b, size(2048.0, 2048.0)), 1089.0);
    }
    assert_eq!(
        frame_tokens(&billing("google", "google-generative-ai", "gemini-3.8-flash"), size(2048.0, 2048.0)),
        1120.0
    );
    for id in ["grok-4.6", "grok-4.7", "grok-4.20-0309-non-reasoning"] {
        let b = billing("xai-oauth", "openai-completions", id);
        assert_eq!(b.rule.as_ref().unwrap()["verified"], true);
        assert_eq!(frame_tokens(&b, size(2048.0, 2048.0)), 2442.0);
    }
    let built = ShapeTarget {
        identity: Some(ModelIdentity {
            class: "anthropic".into(),
            family: Some("opus".into()),
            revision: Some("4.6.0".into()),
        }),
        ..target("house", "openai-completions", "house-vision-model")
    };
    assert_eq!(frame_tokens(&frame_billing(Some(&built)).unwrap(), size(1568.0, 1568.0)), 1521.0);
    let unknown = billing("custom", "some-future-api", "qwen/qwen3-vl");
    assert!(unknown.rule.is_none());
    assert_eq!(frame_tokens(&unknown, size(20.0, 20.0)), 5024.0);
    assert_eq!(frame_billing_key(&unknown), "auto:ceiling");
    for api in ["openai-completions", "openai-responses", "openai-codex-responses", "azure-openai-responses"] {
        let fallback = billing("custom", api, "moonshotai/kimi-k2.6");
        assert_eq!(fallback.image_detail.as_deref(), Some("original"));
        assert_eq!(frame_tokens(&fallback, size(1568.0, 1562.0)), 2882.0);
        assert!(fallback.rule.unwrap().get("verified").is_none());
    }
}

#[test]
fn image_formulas_geometry_fit_and_conservative_planning_match_upstream_examples() {
    observe_added_shape_render_and_planning_contracts();
    let gpt = billing("openai", "openai-responses", "gpt-5.5");
    for (w, h, detail, expected) in [
        (1932.0, 1920.0, "original", 4392.0),
        (1932.0, 1920.0, "high", 2940.0),
        (1932.0, 1920.0, "low", 308.0),
        (1024.0, 1024.0, "high", 1229.0),
        (2048.0, 2048.0, "high", 3000.0),
        (4096.0, 512.0, "original", 2458.0),
    ] {
        let b = FrameBilling { image_detail: Some(detail.into()), ..gpt.clone() };
        assert_eq!(frame_tokens(&b, size(w, h)), expected, "{w}x{h} {detail}");
    }
    assert_eq!(frame_tokens(&billing("openai", "openai-responses", "gpt-6-astra"), size(4096.0, 4096.0)), 19661.0);
    assert_eq!(frame_tokens(&gpt, size(4096.0, 4096.0)), 12000.0);
    let standard = billing("anthropic", "anthropic-messages", "claude-opus-4-6");
    for (w, h, expected) in
        [(1075.0, 1520.0, 1551.0), (1920.0, 1080.0, 1560.0), (3000.0, 2999.0, 1521.0), (2999.0, 3000.0, 1521.0)]
    {
        assert_eq!(frame_tokens(&standard, size(w, h)), expected);
    }
    let cursor = resolve_shape(Some(&target("cursor", "cursor-agent", "claude-opus-5-high")), None).unwrap();
    assert_eq!((cursor.frame_size, cursor.frame_token_estimate), (1024.0, 1369.0));
    assert_eq!(
        resolve_shape(Some(&target("cursor", "cursor-agent", "claude-opus-5-high")), Some("5x8-bw"))
            .unwrap()
            .frame_size,
        1024.0
    );
    assert_eq!(
        resolve_shape(Some(&target("cursor", "cursor-agent", "gemini-3.7-flash")), None).unwrap().frame_size,
        2048.0
    );
    let verified = resolve_shape(Some(&target("openai-codex", "openai-codex-responses", "gpt-6.1-sol")), None).unwrap();
    assert_eq!(planned_frame_tokens(&verified), 2882.0);
    assert_eq!(planned_frame_tokens(&Shape { frame_tokens_verified: Some(false), ..verified.clone() }), 5024.0);
    let unverified = resolve_shape(Some(&target("xai", "openai-responses", "grok-4.5")), None).unwrap();
    assert_eq!(unverified.frame_token_estimate, 2882.0);
    assert_eq!(planned_frame_tokens(&unverified), 5024.0);
    let forced = resolve_shape(Some(&target("xai", "openai-responses", "grok-4.5")), Some("5x8-bw")).unwrap();
    assert_eq!(planned_frame_tokens(&forced), 7874.0);
    let sonnet = resolve_shape(Some(&target("anthropic", "anthropic-messages", "claude-sonnet-4-5")), None).unwrap();
    let opus = resolve_shape(Some(&target("anthropic", "anthropic-messages", "claude-opus-5-5")), None).unwrap();
    assert_eq!(max_frames_for_data_budget(&verified, 3_000_000), 26);
    assert_eq!(max_frames_for_data_budget(&sonnet, 3_000_000), 26);
    assert_eq!(max_frames_for_data_budget(&opus, 3_000_000), 17);
    assert_eq!(max_frames_for_data_budget(&opus, 12_000_000), 70);
    for variant in ["8x13-bw", "6x12-dim", "doc-8on16-sent-dim", "8on16-bw", "silver16-bw"] {
        let inkier = resolve_shape(None, Some(variant)).unwrap();
        assert_eq!(max_frames_for_data_budget(&inkier, 3_000_000), 17);
    }
}

#[test]
fn deployment_policy_official_proxy_provider_type_wire_id_and_read_opt_in() {
    for (provider, api, id, generic, official, bytes) in [
        ("anthropic", "anthropic-messages", "claude-opus-5-5", 90, 600, 12_000_000),
        ("anthropic", "anthropic-messages", "claude-haiku-4-5", 90, 100, 12_000_000),
        ("openai-codex", "openai-codex-responses", "gpt-6.1-sol", 200, 2000, 12_000_000),
        ("openrouter", "openai-completions", "anthropic/claude-opus-5.5", 90, 90, 3_000_000),
        ("cursor", "cursor-agent", "claude-opus-5-high", 20, 20, 3_000_000),
        ("google-antigravity", "google-gemini-cli", "claude-sonnet-4-6", 20, 20, 3_000_000),
        ("xai", "openai-completions", "grok-4.7", 60, 60, 3_000_000),
        ("ramp", "anthropic-messages", "claude-opus-5-5", 5, 5, 3_000_000),
    ] {
        let direct = deployment(provider, api, id, true);
        let proxy = DeploymentTarget { official_endpoint: false, ..direct.clone() };
        assert_eq!(provider_image_budget(Some(&direct)).unwrap(), official, "{provider}/{id}");
        assert_eq!(provider_image_budget(Some(&proxy)).unwrap(), generic, "{provider}/{id}");
        assert_eq!(provider_frame_budget(Some(&direct)).unwrap(), official.min(80));
        assert_eq!(frame_data_bytes_budget(Some(&direct)).unwrap(), bytes, "{provider}/{id}");
        assert_eq!(frame_data_bytes_budget(Some(&proxy)).unwrap(), 3_000_000);
    }
    assert_eq!(provider_image_budget(None).unwrap(), 5);
    assert_eq!(frame_data_bytes_budget(None).unwrap(), 3_000_000);
    let custom_provider = DeploymentTarget {
        provider_type: Some("openai-codex".into()),
        ..deployment("house", "openai-codex-responses", "gpt-6.1-sol", true)
    };
    assert_eq!(provider_image_budget(Some(&custom_provider)).unwrap(), 2000);
    let opaque = DeploymentTarget {
        request_model_id: Some("claude-opus-5-5".into()),
        ..deployment("anthropic", "anthropic-messages", "house-opaque", true)
    };
    assert!(reader_images_read_results(&opaque).unwrap());
    assert!(
        reader_images_read_results(&deployment("anthropic", "anthropic-messages", "claude-opus-5-5", true)).unwrap()
    );
    assert!(
        !reader_images_read_results(&deployment("anthropic", "anthropic-messages", "claude-opus-5-4", true)).unwrap()
    );
    assert!(!reader_images_read_results(&deployment("openai", "openai-responses", "gpt-6.1-sol", true)).unwrap());
}

// Supplement to the existing coarse catalog bank: exact added b91db66e
// shape/render observations, not additional Cargo targets or test functions.
fn observe_added_shape_render_and_planning_contracts() {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    fn decoded(data: &str) -> (usize, usize, Vec<u8>) {
        let bytes = STANDARD.decode(data).unwrap();
        let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::EXPAND);
        let mut reader = decoder.read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut pixels).unwrap();
        pixels.truncate(info.buffer_size());
        (info.width as usize, info.height as usize, pixels)
    }
    let cursor_claude = target("cursor", "cursor-agent", "claude-opus-5-high");
    let claude = resolve_shape(Some(&cursor_claude), None).unwrap();
    let cursor_gpt = resolve_shape(Some(&target("cursor", "cursor-agent", "gpt-5.4-nano")), None).unwrap();
    assert_eq!(
        (cursor_gpt.cell_height, cursor_gpt.frame_size, cursor_gpt.frame_token_estimate),
        (22.0, 1024.0, 1229.0)
    );
    let images = render_many(
        &"cursor ".repeat(2000),
        RenderManyOptions { shape: Some(claude), max_frames: Some(1.0), ..Default::default() },
    )
    .unwrap();
    let HistoryBlock::Image { data, .. } = &images[0] else { panic!("cursor frame") };
    assert_eq!(decoded(data).0, 1024);
    let provider_only = ShapeTarget { api: None, ..cursor_claude.clone() };
    assert_eq!(resolve_shape(Some(&provider_only), None).unwrap().frame_size, 1024.0);
    let api_only = ShapeTarget { provider: None, ..cursor_claude };
    let images = render_many(
        &"cursor ".repeat(2000),
        RenderManyOptions { model: Some(api_only), max_frames: Some(1.0), ..Default::default() },
    )
    .unwrap();
    let HistoryBlock::Image { data, .. } = &images[0] else { panic!("api-only frame") };
    assert_eq!(decoded(data).0, 1024);
    for reader in [
        target("anthropic", "anthropic-messages", "claude-opus-4-6"),
        target("openrouter", "openai-completions", "anthropic/claude-sonnet-4.6"),
        target("amazon-bedrock", "bedrock-converse-stream", "us.anthropic.claude-haiku-4-5-v1:0"),
    ] {
        assert_eq!(resolve_shape(Some(&reader), None).unwrap().frame_token_estimate, 1521.0);
    }
    let built = ShapeTarget {
        identity: Some(ModelIdentity {
            class: "anthropic".into(),
            family: Some("opus".into()),
            revision: Some("4.6.0".into()),
        }),
        ..target("house", "openai-completions", "house-vision-model")
    };
    assert_eq!(resolve_shape(Some(&built), None).unwrap().frame_token_estimate, 1521.0);
    assert_eq!(
        resolve_shape(Some(&target("openrouter", "openrouter", "~anthropic/claude-opus-latest")), None)
            .unwrap()
            .frame_token_estimate,
        3136.0
    );
    assert_eq!(
        resolve_shape(Some(&target("house", "openai-completions", "google/gemini-3.5-flash")), None)
            .unwrap()
            .frame_token_estimate,
        1120.0
    );
    let unknown = resolve_shape(Some(&target("house", "future-api", "qwen/qwen3-vl")), None).unwrap();
    assert_eq!(unknown.frame_token_estimate, FRAME_TOKEN_ESTIMATE as f64);
    for (reader, variant, expected) in [
        (target("anthropic", "anthropic-messages", "claude-opus-5-5"), None, 4761.0),
        (target("anthropic", "anthropic-messages", "claude-opus-4-6"), None, 5024.0),
        (target("xai", "openai-responses", "grok-4.7"), None, 2442.0),
        (target("openai-codex", "openai-codex-responses", "gpt-6.1-sol"), Some("5x8-bw"), 7874.0),
    ] {
        assert_eq!(planned_frame_tokens(&resolve_shape(Some(&reader), variant).unwrap()), expected);
    }
    let gemini = resolve_shape(Some(&target("google", "google-generative-ai", "gemini-3.5-flash")), None).unwrap();
    assert_eq!(max_frames_for_data_budget(&gemini, 3_000_000), 17);
    let sonnet46 = resolve_shape(Some(&target("anthropic", "anthropic-messages", "claude-sonnet-4-6")), None).unwrap();
    assert_eq!((sonnet46.frame_token_estimate, sonnet46.frame_tokens_verified), (1521.0, Some(true)));
    // Added source test compares real short and longer frame prefixes, not only IHDR.
    let shape = resolve_shape(None, Some("8on22-bw")).unwrap();
    let short =
        render_many(&"x".repeat(83), RenderManyOptions { shape: Some(shape.clone()), ..Default::default() }).unwrap();
    let HistoryBlock::Image { data, .. } = &short[0] else { panic!("short frame") };
    let (sw, sh, sp) = decoded(data);
    let longer = render(&format!("{}{}{}", "x".repeat(83), " ".repeat(113), "y".repeat(400)), &shape, None).unwrap();
    let (lw, lh, lp) = decoded(&longer.data);
    assert_eq!((sw, sh, lw, lh), (1568, 64, 1568, 88));
    assert_eq!(&sp[..22 * sw * 3], &lp[..22 * lw * 3]);
    assert!(sp[22 * sw * 3..].iter().all(|&pixel| pixel == 255));
    assert!(
        !reader_includes_thinking(&deployment("anthropic", "anthropic-messages", "claude-opus-5-5", true)).unwrap()
    );
    assert!(
        !reader_includes_thinking(&deployment("openrouter", "openai-completions", "anthropic/claude-opus-5-5", false))
            .unwrap()
    );
    assert!(
        reader_includes_thinking(&deployment("openai-codex", "openai-codex-responses", "gpt-6.1-sol", true)).unwrap()
    );
}
