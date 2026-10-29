//! Fixed OMP shape tables, model identity and provider billing/budgets.
use crate::{
    DeploymentTarget, Error, FrameBilling, IdealShape, ImageSize, NormalizeOptions, Result, Shape, ShapeTarget,
};
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
    for key in ["stretch", "stopwordDim", "frameTokensVerified"] {
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
pub fn frame_billing(target: Option<&ShapeTarget>) -> Result<FrameBilling> {
    let rule = target.map(crate::catalog::image_axes).transpose()?.and_then(|axes| {
        axes.get("imageTokenization")
            .filter(|rule| matches!(rule["regime"].as_str(), Some("openai-patch" | "anthropic-patch" | "fixed")))
            .cloned()
    });
    let image_detail =
        (billing_family(target.and_then(|t| t.api.as_deref())) == "openai").then(|| "original".to_owned());
    Ok(FrameBilling { rule, image_detail })
}
pub fn frame_billing_key(billing: &FrameBilling) -> String {
    format!(
        "{}:{}",
        billing.image_detail.as_deref().unwrap_or("auto"),
        billing.rule.as_ref().map(Value::to_string).unwrap_or_else(|| "ceiling".into())
    )
}
pub fn frame_tokens(billing: &FrameBilling, size: ImageSize) -> f64 {
    billing.rule.as_ref().map_or(crate::FRAME_TOKEN_ESTIMATE as f64, |rule| {
        crate::catalog::image_tokens(rule, size, billing.image_detail.as_deref())
    })
}
pub fn planned_frame_tokens(shape: &Shape) -> f64 {
    if shape.frame_tokens_verified == Some(true) {
        shape.frame_token_estimate
    } else {
        shape.frame_token_estimate.max(crate::FRAME_TOKEN_ESTIMATE as f64)
    }
}
pub(crate) fn price(mut shape: Shape, target: Option<&ShapeTarget>) -> Result<Shape> {
    let billing = frame_billing(target)?;
    shape.frame_token_estimate =
        frame_tokens(&billing, ImageSize { width: shape.frame_size, height: shape.frame_size });
    shape.frame_tokens_verified = billing.rule.as_ref().is_some_and(|rule| rule["verified"] == true).then_some(true);
    shape.image_detail = billing.image_detail;
    Ok(shape)
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
        _ => {
            return Err(Error::from_reason(format!("Unknown snapcompact shape variant {name:?}")));
        }
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
        frame_tokens_verified: None,
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
        return price(fit_host_image_edge(shape_variant(variant)?, model)?, model);
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
    price(fit_host_image_edge(shape, model)?, model)
}
fn fit_host_image_edge(mut shape: Shape, target: Option<&ShapeTarget>) -> Result<Shape> {
    if let Some(edge) = target
        .map(crate::catalog::image_axes)
        .transpose()?
        .and_then(|axes| axes.get("imageMaxEdge").and_then(Value::as_f64).filter(|edge| *edge > 0.0))
        && shape.frame_size > edge
    {
        shape.frame_size = edge;
    }
    Ok(shape)
}
pub fn resolve_shape_for_text(text: &str, model: Option<&ShapeTarget>, variant: Option<&str>) -> Result<Shape> {
    resolve_shape_for_text_utf16(&crate::Utf16Text::from(text), model, variant)
}
pub fn resolve_shape_for_text_utf16(
    text: &crate::Utf16Text,
    model: Option<&ShapeTarget>,
    variant: Option<&str>,
) -> Result<Shape> {
    let shape = resolve_shape(model, variant)?;
    if variant.is_some_and(|v| !v.is_empty() && v != "auto") {
        return Ok(shape);
    }
    let silver = resolve_shape(model, Some("silver16-bw"))?;
    let current_safe =
        crate::scan_renderability_utf16(text, NormalizeOptions { shape: Some(shape.clone()), ..Default::default() })
            .is_safe;
    let chars: Vec<_> = crate::text::normalized_input_utf16(text)
        .into_iter()
        .filter_map(|p| match p {
            crate::Utf16CodePoint::Scalar(c) => Some(c),
            crate::Utf16CodePoint::Lone(_) => None,
        })
        .collect();
    let graphics: Vec<_> = chars
        .into_iter()
        .filter(|&c| {
            !matches!(c, ' ' | crate::DIM_ON | crate::DIM_OFF | crate::NEWLINE_GLYPH) && !crate::text::unrenderable(c)
        })
        .collect();
    let wide = graphics.iter().filter(|&&c| crate::layout::is_wide(c)).count();
    let cjk = wide >= 8 && wide as f64 / graphics.len() as f64 >= 0.25;
    if (!current_safe || shape.font != "silver" && cjk)
        && crate::scan_renderability_utf16(text, NormalizeOptions { shape: Some(silver.clone()), ..Default::default() })
            .is_safe
    {
        Ok(silver)
    } else {
        Ok(shape)
    }
}
pub(crate) fn dense_companion(high: &Shape, _api: Option<&str>) -> Shape {
    if high.columns == Some(2) || high.font == "silver" {
        return high.clone();
    }
    let mut low = shape_variant("8on16-bw").expect("fixed variant");
    low.frame_size = high.frame_size;
    low.frame_token_estimate = high.frame_token_estimate;
    low.image_detail = high.image_detail.clone();
    // Upstream constructs a new geometry and copies only estimate/detail here.
    // It intentionally does not copy frameTokensVerified to the dense tier.
    if crate::geometry(&low, None).capacity > crate::geometry(high, None).capacity { low } else { high.clone() }
}
pub fn provider_image_budget(deployment: Option<&DeploymentTarget>) -> Result<usize> {
    let Some(deployment) = deployment else {
        return Ok(crate::DEFAULT_PROVIDER_IMAGE_BUDGET);
    };
    let axes = crate::catalog::deployment_axes(deployment)?;
    // Source `official && policy.officialImageCountBudget || policy.imageCountBudget`.
    let official = deployment.official_endpoint.then(|| axes.get("officialImageCountBudget")).flatten().filter(
        |value| match value {
            Value::Null => false,
            Value::Bool(v) => *v,
            Value::Number(v) => v.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
            Value::String(v) => !v.is_empty(),
            _ => true,
        },
    );
    let value = official.or_else(|| axes.get("imageCountBudget"));
    Ok(value
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && *v > 0.0 && v.fract() == 0.0)
        .map(|v| v as usize)
        .unwrap_or(crate::DEFAULT_PROVIDER_IMAGE_BUDGET))
}
pub fn provider_frame_budget(deployment: Option<&DeploymentTarget>) -> Result<usize> {
    Ok(provider_image_budget(deployment)?.min(crate::MAX_FRAMES_DEFAULT))
}
pub fn frame_data_bytes_budget(deployment: Option<&DeploymentTarget>) -> Result<usize> {
    let Some(deployment) = deployment.filter(|d| d.official_endpoint) else {
        return Ok(crate::FRAME_DATA_BYTES_BUDGET);
    };
    let axes = crate::catalog::deployment_axes(deployment)?;
    Ok(axes
        .get("inlineImageByteBudget")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|value| (value / 2.0).floor() as usize)
        .unwrap_or(crate::FRAME_DATA_BYTES_BUDGET))
}
pub fn reader_images_read_results(deployment: &DeploymentTarget) -> Result<bool> {
    Ok(crate::catalog::deployment_axes(deployment)?.get("snapcompactImageReads") == Some(&Value::Bool(true)))
}
pub fn reader_includes_thinking(deployment: &DeploymentTarget) -> Result<bool> {
    Ok(crate::catalog::deployment_axes(deployment)?.get("classifierCotExtraction") != Some(&Value::Bool(true)))
}
pub fn max_frames_for_data_budget(shape: &Shape, budget: usize) -> usize {
    let area_priced = ["8on22-bw", "11on16-bw"].iter().any(|name| {
        let variant = shape_variant(name).expect("fixed variant");
        variant.font == shape.font
            && variant.cell_width == shape.cell_width
            && variant.cell_height == shape.cell_height
            && variant.stretch == shape.stretch
            && variant.variant == shape.variant
            && variant.stopword_dim == shape.stopword_dim
            && variant.columns == shape.columns
            && variant.line_repeat == shape.line_repeat
    });
    let area_ratio = if area_priced { (shape.frame_size / 1932.0).powi(2) } else { 1.0 };
    let frame_bytes =
        (crate::FRAME_DATA_BYTES_ESTIMATE as f64).min((crate::FRAME_DATA_BYTES_ESTIMATE as f64 * area_ratio).ceil());
    ((budget as f64 / frame_bytes).floor() as usize).max(1)
}
