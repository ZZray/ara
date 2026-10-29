//! Snapcompact's catalog-axis and image-tokenization dependency closure.
//! Source: OMP b91db66e9b0b65a2c977e6a5bc74b0f4b1d58d1c,
//! packages/catalog/src/compat/{cascade,image-tokenization,request-size,resolve}.ts.
//! MIT; Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE.omp-catalog.
use crate::{DeploymentTarget, Error, ImageSize, Result, ShapeTarget};
use serde_json::{Map, Value};

fn array(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn text(value: &Value) -> &str {
    value.as_str().unwrap_or("")
}
fn number(value: &Value) -> f64 {
    value.as_f64().unwrap_or(f64::NAN)
}

/// A missing rule dimension is unconstrained; an authored empty set never matches.
fn set_matches(rule: &Value, key: &str, value: Option<&str>) -> bool {
    rule.get(key).is_none_or(|set| value.is_some_and(|v| array(set).iter().any(|x| x.as_str() == Some(v))))
}

fn rank(
    rule: &Value,
    target: &ShapeTarget,
    policy_id: &str,
    identity: &crate::identity::Identity,
) -> Option<(u8, u8, i64)> {
    if rule.get("class").is_some_and(|v| v.as_str() != Some(identity.class.as_str()))
        || !set_matches(rule, "providers", Some(target.provider.as_deref().unwrap_or("")))
        || !set_matches(rule, "apis", Some(target.api.as_deref().unwrap_or("")))
        // Neither image-axis nor catalog-policy callers supply an upstream route selector.
        || !set_matches(rule, "upstreams", None)
        || rule.get("family").is_some_and(|v| v.as_str() != identity.family.as_deref())
    {
        return None;
    }
    if let Some(terms) = rule.get("revision") {
        let current = identity.revision?;
        for term in array(terms) {
            let revision = crate::identity::parse_revision(text(&term["revision"]))?;
            let matches = match text(&term["op"]) {
                ">=" => current >= revision,
                ">" => current > revision,
                "<=" => current <= revision,
                "<" => current < revision,
                "=" => current == revision,
                _ => false,
            };
            if !matches {
                return None;
            }
        }
    }
    let mut exactness = 0;
    if let Some(models) = rule.get("models") {
        let lower = policy_id.to_lowercase();
        let mut best = None;
        for selector in array(models) {
            let value = text(&selector["value"]);
            let kind = text(&selector["kind"]);
            let matches = match kind {
                "exact" => policy_id == value,
                "glob" => crate::identity::glob(value, &lower),
                "token" => lower.split(|c: char| !c.is_ascii_alphanumeric()).any(|token| token == value),
                _ => false,
            };
            if matches {
                best = Some(best.unwrap_or(0).max(if kind == "exact" { 2 } else { 1 }));
            }
        }
        exactness = best?;
    }
    let dimensions = ["class", "providers", "apis", "upstreams", "family", "revision", "models"]
        .iter()
        .filter(|key| rule.get(**key).is_some())
        .count() as u8;
    Some((exactness, dimensions, rule["priority"].as_i64().unwrap_or(0)))
}

type Rank = (u8, u8, i64);
type Winners<'a> = Vec<(String, Rank, &'a Value)>;
fn contest<'a>(
    winners: &mut Winners<'a>,
    axis: &str,
    rank: Rank,
    rule: &'a Value,
    target: &ShapeTarget,
    policy_id: &str,
) -> Result<()> {
    if let Some(held) = winners.iter_mut().find(|(key, _, _)| key == axis) {
        if held.1 == rank {
            return Err(Error::from_reason(format!(
                "ambiguous overlap for `{}/{}` on axis `{axis}`: rules `{}` and `{}` tie; add an explicit priority",
                target.provider.as_deref().unwrap_or(""),
                policy_id,
                text(&held.2["source"]),
                text(&rule["source"])
            )));
        }
        if held.1 > rank {
            return Ok(());
        }
        *held = (axis.to_owned(), rank, rule);
    } else {
        winners.push((axis.to_owned(), rank, rule));
    }
    Ok(())
}
/// Resolve the complete catalog bag. Other cascade groups are also contested so
/// their source ambiguity errors cannot be hidden by this catalog-only consumer.
fn axes(
    target: &ShapeTarget,
    policy_id: &str,
    identity: &crate::identity::Identity,
    initial_reasoning: bool,
) -> Result<Map<String, Value>> {
    let mut ranked = Vec::new();
    for rule in array(&crate::identity::rules()["cascade"]["rules"]) {
        let Some(rank) = rank(rule, target, policy_id, identity) else {
            continue;
        };
        ranked.push((rule, rank));
    }
    let mut reasoning = initial_reasoning;
    if !reasoning {
        let mut upgrade = Vec::new();
        let mut has_efforts = false;
        for &(rule, rank) in &ranked {
            let Some(thinking) = rule["thinking"].as_object() else {
                continue;
            };
            if thinking.contains_key("upgradeNeutral") {
                contest(&mut upgrade, "upgradeNeutral", rank, rule, target, policy_id)?;
            }
            if thinking.contains_key("efforts") {
                has_efforts = true;
                if rank.0 == 2 {
                    reasoning = true;
                }
            }
        }
        if let Some((_, _, rule)) = upgrade.first() {
            let identity_scoped = rule.get("class").is_some_and(|class| class != "unknown")
                || ["family", "revision", "models"].iter().any(|key| rule.get(*key).is_some());
            if identity_scoped && rule["thinking"]["upgradeNeutral"] == true && has_efforts {
                reasoning = true;
            }
        }
    }
    let offers_thinking = reasoning || identity.class == "unknown";
    let (mut wire, mut thinking, mut catalog) = (Vec::new(), Vec::new(), Vec::new());
    for (rule, rank) in ranked {
        for (group, winners, enabled) in
            [("wire", &mut wire, true), ("catalog", &mut catalog, true), ("thinking", &mut thinking, offers_thinking)]
        {
            if !enabled {
                continue;
            }
            if let Some(values) = rule[group].as_object() {
                for axis in values.keys() {
                    contest(winners, axis, rank, rule, target, policy_id)?;
                }
            }
        }
    }
    Ok(catalog
        .into_iter()
        .map(|(axis, _, rule)| {
            let value = rule["catalog"][&axis].clone();
            (axis, value)
        })
        .collect())
}

pub(crate) fn image_axes(target: &ShapeTarget) -> Result<Map<String, Value>> {
    let id = target.id.as_deref().unwrap_or("");
    let identity = if let Some(identity) = &target.identity {
        crate::identity::Identity {
            class: identity.class.clone(),
            family: identity.family.clone(),
            revision: identity.revision.as_deref().and_then(crate::identity::parse_revision),
        }
    } else if id.is_empty() {
        crate::identity::Identity { class: "unknown".into(), ..Default::default() }
    } else {
        crate::identity::classify_for_provider(target.provider.as_deref().unwrap_or(""), id, true)?
    };
    axes(target, id, &identity, false)
}

/// Mirrors resolveCatalogAxes(toModelSpec(model)); built identity is deliberately recomputed here.
pub(crate) fn deployment_axes(deployment: &DeploymentTarget) -> Result<Map<String, Value>> {
    let model = &deployment.model;
    let provider = model.provider.as_deref().unwrap_or("");
    let id = model.id.as_deref().unwrap_or("");
    let identity = crate::identity::classify_for_provider(provider, id, false)?;
    let (policy_id, identity) = if let Some(wire_id) = deployment.request_model_id.as_deref().filter(|wire| *wire != id)
        && identity.class == "unknown"
    {
        let wire_identity = crate::identity::classify_for_provider(provider, wire_id, false)?;
        if wire_identity.class == "unknown" { (id, identity) } else { (wire_id, wire_identity) }
    } else {
        (id, identity)
    };
    let mut target = model.clone();
    target.provider = Some(deployment.provider_type.as_deref().unwrap_or(provider).into());
    axes(&target, policy_id, &identity, deployment.reasoning)
}

fn fit_long_edge(size: ImageSize, max_edge: f64) -> ImageSize {
    let longest = size.width.max(size.height);
    if longest <= max_edge {
        return size;
    }
    let scale = max_edge / longest;
    ImageSize { width: (size.width * scale).round().max(1.0), height: (size.height * scale).round().max(1.0) }
}
fn openai_patches(size: ImageSize, sizing: &Value) -> f64 {
    let ImageSize { width, height } = fit_long_edge(size, number(&sizing["maxEdge"]));
    let patches = (width / 32.0).ceil() * (height / 32.0).ceil();
    let Some(budget) = sizing.get("patchBudget").map(number) else {
        return patches;
    };
    if patches <= budget {
        return patches;
    }
    let shrink = (32.0 * 32.0 * budget / (width * height)).sqrt();
    let scaled_w = width * shrink / 32.0;
    let scaled_h = height * shrink / 32.0;
    let adjusted = shrink * (scaled_w.floor() / scaled_w).min(scaled_h.floor() / scaled_h);
    let resized_w = (width * adjusted).floor();
    let resized_h = (height * adjusted).floor();
    if resized_w <= 0.0 || resized_h <= 0.0 {
        return budget;
    }
    budget.min((resized_w / 32.0).ceil() * (resized_h / 32.0).ceil())
}
fn anthropic_tokens(size: ImageSize, max_edge: f64, max_tokens: f64) -> f64 {
    let long = size.width.max(size.height);
    let short_side = size.width.min(size.height);
    let aspect = long / short_side;
    let patches = |l: f64, s: f64| (l / 28.0).ceil() * (s / 28.0).ceil();
    let fits = |l: f64, s: f64| {
        (l / 28.0).ceil() * 28.0 <= max_edge && (s / 28.0).ceil() * 28.0 <= max_edge && patches(l, s) <= max_tokens
    };
    let short = |l: f64| (l / aspect).round_ties_even().max(1.0);
    if fits(long, short_side) {
        return patches(long, short_side);
    }
    let (mut lo, mut hi) = (1.0, long);
    while lo + 1.0 < hi {
        let mid = ((lo + hi) / 2.0).floor();
        if fits(mid, short(mid)) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    patches(lo, short(lo))
}
pub(crate) fn image_tokens(rule: &Value, size: ImageSize, detail: Option<&str>) -> f64 {
    match text(&rule["regime"]) {
        "fixed" => number(&rule["tokens"]),
        "openai-patch" => {
            let level = match detail {
                Some("low") => "low",
                Some("high") => "high",
                Some("original") => "original",
                _ => text(&rule["auto"]),
            };
            (openai_patches(size, &rule[level]) * number(&rule["multiplier"])).ceil()
        }
        "anthropic-patch" => anthropic_tokens(size, number(&rule["maxEdge"]), number(&rule["maxTokens"])),
        _ => f64::NAN,
    }
}
