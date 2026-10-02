//! Fixed OMP shape tables, model identity and provider billing/budgets.
use crate::{Error, IdealShape, NormalizeOptions, Result, Shape, ShapeTarget};
use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

pub const SHAPE_VARIANT_NAMES: &[&str] = &[
    "8x8r-bw",
    "8x8r-sent",
    "8x8u-bw",
    "8x8u-sent",
    "6x6u-bw",
    "6x6u-sent",
    "5x8-bw",
    "5x8-sent",
    "6x12-dim",
    "8x13-bw",
    "8on16-bw",
    "8on22-bw",
    "11on16-bw",
    "silver16-bw",
    "doc-8on16-bw",
    "doc-8on16-sent",
    "doc-8on16-sent-dim",
];
pub fn is_shape_variant_name(value: &str) -> bool {
    SHAPE_VARIANT_NAMES.contains(&value)
}
pub fn is_shape(value: &Value) -> bool {
    for key in ["stretch", "stopwordDim"] {
        if value.get(key).is_some_and(|v| !v.is_boolean()) {
            return false;
        }
    }
    if value.get("columns").is_some_and(|v| v != 1 && v != 2) {
        return false;
    }
    if value
        .get("imageDetail")
        .is_some_and(|v| !v.as_str().is_some_and(|d| matches!(d, "auto" | "low" | "high" | "original")))
    {
        return false;
    }
    serde_json::from_value::<Shape>(value.clone()).ok().is_some_and(|s| {
        matches!(s.font.as_str(), "5x8" | "8x8" | "6x12" | "8x13" | "silver")
            && matches!(s.variant.as_str(), "sent" | "bw")
            && s.cell_width > 0.0
            && s.cell_height > 0.0
            && s.line_repeat > 0.0
            && s.frame_size > 0.0
            && s.frame_token_estimate > 0.0
            && s.columns.is_none_or(|c| c == 1 || c == 2)
            && s.image_detail.as_deref().is_none_or(|d| matches!(d, "auto" | "low" | "high" | "original"))
    })
}
pub(crate) fn billing_family(api: Option<&str>) -> &'static str {
    match api {
        Some("anthropic-messages" | "bedrock-converse-stream") => "anthropic",
        Some("openai-completions" | "openai-responses" | "openai-codex-responses" | "azure-openai-responses") => {
            "openai"
        }
        Some("google-generative-ai" | "google-gemini-cli" | "google-vertex") => "google",
        _ => "unknown",
    }
}
pub(crate) fn price(mut shape: Shape, api: Option<&str>) -> Shape {
    shape.image_detail = None;
    shape.frame_token_estimate = match billing_family(api) {
        "google" => 1120.0,
        "openai" => {
            shape.image_detail = Some("original".into());
            ((shape.frame_size / 32.0).ceil().powi(2).min(10000.0) * 1.2).ceil()
        }
        _ => ((shape.frame_size / 28.0).ceil().powi(2).min(4784.0) * 1.05).ceil(),
    };
    shape
}
pub fn shape_variant(name: &str) -> Result<Shape> {
    let (font, cw, ch, repeat, size) = match name {
        "8x8r-bw" | "8x8r-sent" => ("8x8", 8, 8, 2, 1568),
        "8x8u-bw" | "8x8u-sent" => ("8x8", 8, 8, 1, 1568),
        "6x6u-bw" | "6x6u-sent" => ("8x8", 6, 6, 1, 1568),
        "5x8-bw" | "5x8-sent" => ("5x8", 5, 8, 1, 2576),
        "6x12-dim" => ("6x12", 6, 12, 1, 1568),
        "8x13-bw" => ("8x13", 8, 13, 1, 1568),
        "8on16-bw" | "doc-8on16-bw" | "doc-8on16-sent" | "doc-8on16-sent-dim" => ("8x13", 8, 16, 1, 1568),
        "8on22-bw" => ("8x13", 8, 22, 1, 1568),
        "11on16-bw" => ("8x13", 11, 16, 1, 1568),
        "silver16-bw" => ("silver", 16, 16, 1, 1568),
        _ => return Err(Error::from_reason(format!("Unknown snapcompact shape variant {name:?}"))),
    };
    Ok(Shape {
        font: font.into(),
        cell_width: f64::from(cw),
        cell_height: f64::from(ch),
        stretch: name.contains("on").then_some(false),
        variant: if name.contains("sent") { "sent" } else { "bw" }.into(),
        stopword_dim: name.ends_with("dim").then_some(true),
        columns: name.starts_with("doc-").then_some(2),
        line_repeat: f64::from(repeat),
        frame_size: f64::from(size),
        frame_token_estimate: 1.0,
        image_detail: None,
    })
}
pub fn ideal_shape_variant(id: &str) -> Option<IdealShape> {
    let identity = crate::identity::classify(id);
    if identity.class == "anthropic"
        && (matches!(identity.family.as_deref(), Some("fable" | "mythos"))
            || identity.family.as_deref() == Some("opus") && identity.revision.is_some_and(|r| r >= [4, 7, 0]))
    {
        return Some(IdealShape { variant: "11on16-bw", frame_size: Some(1932) });
    }
    static MODEL_VARIANTS: OnceLock<Vec<(Regex, IdealShape)>> = OnceLock::new();
    MODEL_VARIANTS
        .get_or_init(|| {
            [
                (r"(?i)claude.*(fable|mythos)", "11on16-bw", Some(1932)),
                (r"(?i)claude", "11on16-bw", None),
                (r"(?i)gemini", "8on22-bw", Some(2048)),
                (r"(?i)gpt|codex", "8on22-bw", None),
                (r"(?i)kimi", "8on22-bw", None),
                (r"(?i)glm", "8on16-bw", None),
            ]
            .into_iter()
            .map(|(pattern, variant, frame_size)| {
                (Regex::new(pattern).expect("fixed model-shape regex"), IdealShape { variant, frame_size })
            })
            .collect()
        })
        .iter()
        .find(|(pattern, _)| pattern.is_match(id))
        .map(|(_, ideal)| ideal.clone())
}
pub fn resolve_shape(model: Option<&ShapeTarget>, variant: Option<&str>) -> Result<Shape> {
    let api = model.and_then(|m| m.api.as_deref());
    if let Some(variant) = variant.filter(|&v| v != "auto" && !v.is_empty()) {
        return Ok(price(shape_variant(variant)?, api));
    }
    let ideal = model.and_then(|m| m.id.as_deref()).filter(|s| !s.is_empty()).and_then(ideal_shape_variant);
    let name = ideal.as_ref().map(|i| i.variant).unwrap_or(if billing_family(api) == "anthropic" {
        "11on16-bw"
    } else {
        "8on22-bw"
    });
    let mut shape = shape_variant(name)?;
    if let Some(size) = ideal.and_then(|i| i.frame_size) {
        shape.frame_size = f64::from(size);
    }
    Ok(price(shape, api))
}
pub fn resolve_shape_for_text(text: &str, model: Option<&ShapeTarget>, variant: Option<&str>) -> Result<Shape> {
    let shape = resolve_shape(model, variant)?;
    if variant.is_some_and(|v| !v.is_empty() && v != "auto") {
        return Ok(shape);
    }
    let silver = resolve_shape(model, Some("silver16-bw"))?;
    let current_safe =
        crate::scan_renderability(text, NormalizeOptions { shape: Some(shape.clone()), ..Default::default() }).is_safe;
    let chars = crate::text::normalized_input_chars(text);
    let graphics: Vec<_> = chars
        .into_iter()
        .filter(|&c| {
            !matches!(c, ' ' | crate::DIM_ON | crate::DIM_OFF | crate::NEWLINE_GLYPH) && !crate::text::unrenderable(c)
        })
        .collect();
    let wide = graphics.iter().filter(|&&c| crate::layout::is_wide(c)).count();
    let cjk = wide >= 8 && wide as f64 / graphics.len() as f64 >= 0.25;
    if (!current_safe || shape.font != "silver" && cjk)
        && crate::scan_renderability(text, NormalizeOptions { shape: Some(silver.clone()), ..Default::default() })
            .is_safe
    {
        Ok(silver)
    } else {
        Ok(shape)
    }
}
pub(crate) fn dense_companion(high: &Shape, api: Option<&str>) -> Shape {
    if high.columns == Some(2) || high.font == "silver" {
        return high.clone();
    }
    let mut low = shape_variant("8on16-bw").expect("fixed variant");
    low.frame_size = high.frame_size;
    let low = price(low, api);
    if crate::geometry(&low, None).capacity > crate::geometry(high, None).capacity { low } else { high.clone() }
}
pub fn provider_image_budget(provider: Option<&str>) -> usize {
    match provider {
        Some("anthropic" | "amazon-bedrock" | "openrouter") => 90,
        Some("openai" | "openai-codex" | "google" | "google-vertex" | "google-gemini-cli") => 200,
        Some("umans") => 10,
        _ => crate::DEFAULT_PROVIDER_IMAGE_BUDGET,
    }
}
pub fn provider_frame_budget(provider: Option<&str>) -> usize {
    provider_image_budget(provider).min(crate::MAX_FRAMES_DEFAULT)
}
pub fn max_frames_for_data_budget(bytes: Option<usize>) -> usize {
    (bytes.unwrap_or(crate::FRAME_DATA_BYTES_BUDGET) / crate::FRAME_DATA_BYTES_ESTIMATE).max(1)
}
