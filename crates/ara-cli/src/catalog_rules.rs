//! Fixed OMP revision, cascade and taxonomy primitives.
//! Source: packages/catalog/src/compat/{revision,cascade,taxonomy}.ts at
//! 596f2da7101178214aa27a753529d15e6b7ad91d. MIT, Copyright (c) 2025 Mario
//! Zechner, 2025-2026 Can Bölük, 2026 Stencil Labs, Inc.; LICENSE.omp-catalog.
//! This module consumes the complete compiled tree without recompiling it.
//! It does not perform the separate collapse.ts model-list transformation.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{error::Error, fmt, sync::OnceLock};

pub const RULES_SHA256: &str = "9ae6cc8f5c0fb2503d7e6d5ef885c01b8d767f1f14281c4432b09efa862d4bcf";
pub const RULES_JSON: &[u8] = include_bytes!("../data/omp-compat-rules.json");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogPolicyError(pub String);
impl fmt::Display for CatalogPolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl Error for CatalogPolicyError {}

pub fn compiled_rules() -> &'static Value {
    static RULES: OnceLock<Value> = OnceLock::new();
    RULES.get_or_init(|| serde_json::from_slice(RULES_JSON).expect("pinned compiled rules are valid JSON"))
}

pub type Revision = [u8; 3];
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RevisionOp {
    #[serde(rename = ">=")]
    GreaterEqual,
    #[serde(rename = ">")]
    Greater,
    #[serde(rename = "<=")]
    LessEqual,
    #[serde(rename = "<")]
    Less,
    #[serde(rename = "=")]
    Equal,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionTerm {
    pub op: RevisionOp,
    pub revision: Revision,
}

fn parse_component(value: &str) -> Option<u8> {
    if value.is_empty() {
        return None;
    }
    let mut out = 0u16;
    for c in value.bytes() {
        if !c.is_ascii_digit() {
            return None;
        }
        out = out * 10 + u16::from(c - b'0');
        if out > 255 {
            return None;
        }
    }
    Some(out as u8)
}

pub fn parse_revision(value: &str) -> Option<Revision> {
    let mut out = [0; 3];
    for (count, part) in value.split(['.', '-']).enumerate() {
        if count == 3 {
            return None;
        }
        out[count] = parse_component(part)?;
    }
    Some(out)
}

pub fn parse_revision_prefix(value: &str) -> Option<Revision> {
    let mut out = [0; 3];
    let mut index = 0;
    let bytes = value.as_bytes();
    for count in 0..3 {
        let start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        let component = if bytes.get(index).is_some_and(u8::is_ascii_alphabetic) {
            None
        } else {
            parse_component(&value[start..index])
        };
        let Some(component) = component else { return (count > 0).then_some(out) };
        out[count] = component;
        if !matches!(bytes.get(index), Some(b'.' | b'-')) || !bytes.get(index + 1).is_some_and(u8::is_ascii_digit) {
            break;
        }
        index += 1;
    }
    Some(out)
}

pub fn compare_revision(a: Revision, b: Revision) -> i16 {
    for i in 0..3 {
        let difference = i16::from(a[i]) - i16::from(b[i]);
        if difference != 0 {
            return difference;
        }
    }
    0
}
pub fn format_revision(revision: Revision) -> String {
    format!("{}.{}.{}", revision[0], revision[1], revision[2])
}
pub fn revision_satisfies(revision: Revision, terms: &[RevisionTerm]) -> bool {
    terms.iter().all(|term| {
        let cmp = compare_revision(revision, term.revision);
        match term.op {
            RevisionOp::GreaterEqual => cmp >= 0,
            RevisionOp::Greater => cmp > 0,
            RevisionOp::LessEqual => cmp <= 0,
            RevisionOp::Less => cmp < 0,
            RevisionOp::Equal => cmp == 0,
        }
    })
}
pub fn parse_revision_constraint(expression: &str) -> Option<Vec<RevisionTerm>> {
    let mut terms = Vec::new();
    // ECMAScript whitespace, including BOM but not U+0085.
    for raw in expression.split(|c: char| ara_prompt::js::trim(&c.to_string()).is_empty()) {
        if raw.is_empty() {
            continue;
        }
        let (op, operand) = if let Some(v) = raw.strip_prefix(">=") {
            (RevisionOp::GreaterEqual, v)
        } else if let Some(v) = raw.strip_prefix("<=") {
            (RevisionOp::LessEqual, v)
        } else if let Some(v) = raw.strip_prefix('>') {
            (RevisionOp::Greater, v)
        } else if let Some(v) = raw.strip_prefix('<') {
            (RevisionOp::Less, v)
        } else {
            (RevisionOp::Equal, raw.strip_prefix('=')?)
        };
        if operand.contains('-') {
            return None;
        }
        terms.push(RevisionTerm { op, revision: parse_revision(operand)? });
    }
    (!terms.is_empty()).then_some(terms)
}

/// Anchored wildcard matching; callers provide lowercased operands when needed.
pub fn glob_match(pattern: &str, value: &str) -> bool {
    let segments: Vec<_> = pattern.split('*').collect();
    if segments.len() == 1 {
        return value == pattern;
    }
    let Some(mut remainder) = value.strip_prefix(segments[0]) else { return false };
    for segment in &segments[1..segments.len() - 1] {
        if segment.is_empty() {
            continue;
        }
        let Some(found) = remainder.find(segment) else { return false };
        remainder = &remainder[found + segment.len()..];
    }
    remainder.ends_with(segments.last().expect("nonempty segments"))
}

#[derive(Clone, Deserialize)]
struct Selector {
    kind: String,
    value: String,
}
#[derive(Clone, Deserialize)]
struct CompiledTerm {
    op: RevisionOp,
    revision: String,
}
#[derive(Clone, Deserialize)]
struct CompiledRule {
    source: String,
    class: Option<String>,
    providers: Option<Vec<String>>,
    family: Option<String>,
    revision: Option<Vec<CompiledTerm>>,
    models: Option<Vec<Selector>>,
    priority: Option<f64>,
    wire: Option<Map<String, Value>>,
    thinking: Option<Map<String, Value>>,
    catalog: Option<Map<String, Value>>,
}
struct IndexedRule {
    compiled: CompiledRule,
    revision: Option<Vec<RevisionTerm>>,
    dimensions: u8,
    priority: f64,
    exact_efforts: bool,
}

fn build_rule_index(cascade: &Value) -> Result<Vec<IndexedRule>, CatalogPolicyError> {
    let rules: Vec<CompiledRule> = serde_json::from_value(cascade.get("rules").cloned().unwrap_or(Value::Null))
        .map_err(|e| CatalogPolicyError(format!("invalid compiled cascade: {e}")))?;
    rules
        .into_iter()
        .map(|compiled| {
            let revision = compiled
                .revision
                .as_ref()
                .map(|terms| {
                    terms
                        .iter()
                        .map(|term| {
                            let revision = parse_revision(&term.revision).ok_or_else(|| {
                                CatalogPolicyError(format!("invalid compiled revision term in {}", compiled.source))
                            })?;
                            Ok(RevisionTerm { op: term.op, revision })
                        })
                        .collect::<Result<Vec<_>, CatalogPolicyError>>()
                })
                .transpose()?;
            let dimensions = [
                compiled.class.is_some(),
                compiled.providers.is_some(),
                compiled.family.is_some(),
                compiled.revision.is_some(),
                compiled.models.is_some(),
            ]
            .into_iter()
            .filter(|v| *v)
            .count() as u8;
            let priority = compiled.priority.unwrap_or(0.0);
            let exact_efforts = compiled.thinking.as_ref().is_some_and(|thinking| thinking.contains_key("efforts"));
            Ok(IndexedRule { compiled, revision, dimensions, priority, exact_efforts })
        })
        .collect()
}

#[derive(Deserialize)]
struct ResolveTarget {
    provider: String,
    class: String,
    family: Option<String>,
    revision: Option<String>,
    model: String,
    reasoning: bool,
}
#[derive(Clone, Copy, PartialEq, PartialOrd)]
struct Rank(u8, u8, f64);
fn rank_rule(rule: &IndexedRule, target: &ResolveTarget, revision: Option<Revision>, lower: &str) -> Option<Rank> {
    let c = &rule.compiled;
    if c.class.as_ref().is_some_and(|class| class != &target.class)
        || c.providers.as_ref().is_some_and(|providers| !providers.contains(&target.provider))
        || c.family.as_ref().is_some_and(|family| Some(family) != target.family.as_ref())
    {
        return None;
    }
    if let Some(terms) = &rule.revision
        && !revision.is_some_and(|revision| revision_satisfies(revision, terms))
    {
        return None;
    }
    let mut exactness = 0;
    if let Some(selectors) = &c.models {
        let mut best = None;
        for selector in selectors {
            let matched = match selector.kind.as_str() {
                "exact" => selector.value == target.model,
                "glob" => glob_match(&selector.value, lower),
                "token" => lower.split(|c: char| !c.is_ascii_alphanumeric()).any(|token| token == selector.value),
                _ => false,
            };
            if matched {
                best = Some(best.unwrap_or(0).max(if selector.kind == "exact" { 2 } else { 1 }));
            }
        }
        exactness = best?;
    }
    Some(Rank(exactness, rule.dimensions, rule.priority))
}

// A Vec retains JS object's first-insertion order when a later rule replaces
// an axis; collection then applies JS integer-key order through js::entries.
type Winners<'a> = Vec<(String, Rank, &'a IndexedRule)>;
fn contest<'a>(
    winners: &mut Winners<'a>,
    axes: Option<&Map<String, Value>>,
    rank: Rank,
    rule: &'a IndexedRule,
    target: &ResolveTarget,
) -> Result<(), CatalogPolicyError> {
    let Some(axes) = axes else { return Ok(()) };
    for (axis, _) in ara_prompt::js::entries(axes) {
        if let Some(held) = winners.iter_mut().find(|(name, _, _)| name == axis) {
            if held.1 == rank {
                return Err(CatalogPolicyError(format!(
                    "ambiguous overlap for `{}/{}` on axis `{axis}`: rules `{}` and `{}` tie; add an explicit priority",
                    target.provider, target.model, held.2.compiled.source, rule.compiled.source
                )));
            }
            if held.1 > rank {
                continue;
            }
            *held = (axis.clone(), rank, rule);
        } else {
            winners.push((axis.clone(), rank, rule));
        }
    }
    Ok(())
}
fn collect(winners: Winners<'_>, pick: impl Fn(&CompiledRule) -> &Option<Map<String, Value>>) -> Value {
    let mut out = Map::new();
    for (axis, _, rule) in winners {
        out.insert(axis.clone(), pick(&rule.compiled).as_ref().unwrap()[&axis].clone());
    }
    Value::Object(ara_prompt::js::entries(&out).into_iter().map(|(k, v)| (k.clone(), v.clone())).collect())
}

pub fn resolve_cascade(target: &Value) -> Result<Value, CatalogPolicyError> {
    static INDEX: OnceLock<Result<Vec<IndexedRule>, CatalogPolicyError>> = OnceLock::new();
    let index = INDEX.get_or_init(|| build_rule_index(&compiled_rules()["cascade"])).as_ref().map_err(Clone::clone)?;
    resolve_over_index(index, target)
}
pub fn resolve_cascade_rules(cascade: &Value, target: &Value) -> Result<Value, CatalogPolicyError> {
    resolve_over_index(&build_rule_index(cascade)?, target)
}
fn resolve_over_index(index: &[IndexedRule], target: &Value) -> Result<Value, CatalogPolicyError> {
    let target: ResolveTarget = serde_json::from_value(target.clone())
        .map_err(|e| CatalogPolicyError(format!("invalid resolve target: {e}")))?;
    let lower = target.model.to_lowercase();
    let revision = target.revision.as_deref().and_then(parse_revision);
    let reasoning = target.reasoning
        || index.iter().any(|rule| {
            rule.exact_efforts && rank_rule(rule, &target, revision, &lower).is_some_and(|rank| rank.0 == 2)
        });
    let mut wire = Vec::new();
    let mut thinking = Vec::new();
    let mut catalog = Vec::new();
    for rule in index {
        let Some(rank) = rank_rule(rule, &target, revision, &lower) else { continue };
        contest(&mut wire, rule.compiled.wire.as_ref(), rank, rule, &target)?;
        contest(&mut catalog, rule.compiled.catalog.as_ref(), rank, rule, &target)?;
        if reasoning {
            contest(&mut thinking, rule.compiled.thinking.as_ref(), rank, rule, &target)?;
        }
    }
    Ok(
        json!({"wire": collect(wire, |r| &r.wire), "thinking": collect(thinking, |r| &r.thinking), "catalog": collect(catalog, |r| &r.catalog)}),
    )
}

#[derive(Clone, Copy, Default, Debug)]
pub struct ClassifyOptions {
    pub observed_at_ms: Option<f64>,
    pub lenient: bool,
}

fn array(v: &Value) -> &[Value] {
    v.as_array().expect("compiled array")
}
fn string(v: &Value) -> &str {
    v.as_str().expect("compiled string")
}
fn contains(v: &Value, s: &str) -> bool {
    array(v).iter().any(|v| v.as_str() == Some(s))
}
fn bare_of(id: &str) -> &str {
    id.rsplit('/').next().unwrap_or(id)
}
fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}
fn js_slice(s: &str, start: usize, end: usize) -> String {
    let units: Vec<_> = s.encode_utf16().collect();
    String::from_utf16(&units[start.min(units.len())..end.min(units.len()).max(start.min(units.len()))])
        .expect("fixed ASCII suffix slicing preserves UTF-16 scalar boundaries")
}
// A suffix probe can start inside a surrogate pair even when no suffix
// matches. JS retains that lone surrogate, which cannot equal the declared
// ASCII suffix. Decode only for comparison, never replace malformed units.
fn suffix_matches_at(model: &str, split: usize, suffix: &str) -> bool {
    let units: Vec<_> = model.encode_utf16().collect();
    String::from_utf16(&units[split.min(units.len())..]).is_ok_and(|tail| tail.to_lowercase() == suffix)
}
fn bounded_match(value: &str, token: &str) -> bool {
    value == token
        || value
            .strip_prefix(token)
            .and_then(|tail| tail.as_bytes().first())
            .is_some_and(|c| c.is_ascii_digit() || matches!(c, b'-' | b'_' | b'.' | b':'))
}
fn matcher_matches(matcher: &Value, lower: &str, bare: &str) -> bool {
    let token = string(&matcher["token"]);
    match string(&matcher["kind"]) {
        "exact" => bare == token,
        "bounded" => bounded_match(bare, token),
        "namespace" => {
            let bounded = matcher["bounded"] == true;
            lower
                .split(|c| c == '/' || (bounded && matches!(c, '.' | ':')))
                .filter(|p| !p.is_empty())
                .any(|p| if bounded { bounded_match(p, token) } else { p == token })
        }
        "prefix" => bare.starts_with(token),
        "glob" => glob_match(token, bare),
        _ => false,
    }
}

type IdentityRank = (f64, usize);
type FamilyRanking<'a> = (Option<(IdentityRank, &'a str)>, Option<(&'a str, &'a str)>);
fn rank_families<'a>(class: &'a Value, subject: &str) -> FamilyRanking<'a> {
    let mut winner: Option<(IdentityRank, &str)> = None;
    let mut tied = None;
    for family in array(&class["families"]) {
        let glob = string(&family["glob"]);
        if !glob_match(glob, subject) {
            continue;
        }
        let rank = (
            family["priority"].as_f64().expect("compiled priority"),
            glob.encode_utf16().filter(|c| *c != u16::from(b'*')).count(),
        );
        let id = string(&family["id"]);
        if let Some((held_rank, held_id)) = winner {
            if held_rank == rank && held_id != id {
                tied = Some((held_id, id));
            } else if held_rank < rank {
                winner = Some((rank, id));
                tied = None;
            }
        } else {
            winner = Some((rank, id));
            tied = None;
        }
    }
    (winner, tied)
}
fn ambiguity(model: &str, first: &str, second: &str, kind: &str) -> CatalogPolicyError {
    CatalogPolicyError(format!("ambiguous {kind} for `{model}`: `{first}` and `{second}` tie"))
}
fn classify_family(
    class: &Value,
    bare: &str,
    model: &str,
    lenient: bool,
) -> Result<Option<String>, CatalogPolicyError> {
    let (winner, tied) = rank_families(class, bare);
    let Some((first, second)) = tied else { return Ok(winner.map(|(_, id)| id.to_owned())) };
    if let Some(separator) = bare.find(['.', ':'])
        && separator > 0
        && separator < bare.len() - 1
        && array(&class["matchers"]).iter().any(|m| m["token"].as_str() == Some(&bare[..separator]))
    {
        let (winner, tied) = rank_families(class, &bare[separator + 1..]);
        if tied.is_none() && winner.is_some() {
            return Ok(winner.map(|(_, id)| id.to_owned()));
        }
    }
    if lenient { Ok(None) } else { Err(ambiguity(model, first, second, "family")) }
}
fn extract_revision(class: &Value, bare: &str) -> Option<String> {
    if contains(&class["skipBare"], bare) {
        return None;
    }
    for rule in array(&class["revisionPrefixes"]) {
        let prefix = string(&rule["prefix"]);
        let tail = if rule["anywhere"] == true {
            bare.find(prefix).map(|i| &bare[i + prefix.len()..])
        } else {
            bare.strip_prefix(prefix)
        };
        let Some(tail) = tail else { continue };
        let digit = tail.find(|c: char| c.is_ascii_digit())?;
        return parse_revision_prefix(&tail[digit..]).map(format_revision);
    }
    None
}
fn ranks_in_class(
    classes: &[Value],
    id: &str,
    model: &str,
    lenient: bool,
) -> Result<Map<String, Value>, CatalogPolicyError> {
    let mut out = Map::new();
    let Some(class) = classes.iter().find(|class| class["id"].as_str() == Some(id)) else { return Ok(out) };
    let lower = ara_prompt::js::trim(model).to_lowercase();
    let bare = bare_of(&lower);
    if let Some(family) = classify_family(class, bare, &lower, lenient)? {
        out.insert("family".into(), family.into());
    }
    if let Some(revision) = extract_revision(class, bare) {
        out.insert("revision".into(), revision.into());
    }
    Ok(out)
}
fn classify_ranks(classes: &[Value], model: &str, lenient: bool) -> Result<Map<String, Value>, CatalogPolicyError> {
    let lower = ara_prompt::js::trim(model).to_lowercase();
    let bare = bare_of(&lower);
    let mut winner: Option<((u8, usize), &Value)> = None;
    let mut tied = None;
    for class in classes {
        for matcher in array(&class["matchers"]) {
            if !matcher_matches(matcher, &lower, bare) {
                continue;
            }
            let rank = (
                match string(&matcher["kind"]) {
                    "exact" => 4,
                    "bounded" => 3,
                    "namespace" => 2,
                    "prefix" => 1,
                    _ => 0,
                },
                js_len(string(&matcher["token"])),
            );
            if let Some((held_rank, held)) = winner {
                if held_rank == rank && held["id"] != class["id"] {
                    tied = Some((string(&held["id"]), string(&class["id"])));
                } else if held_rank < rank {
                    winner = Some((rank, class));
                    tied = None;
                }
            } else {
                winner = Some((rank, class));
                tied = None;
            }
        }
    }
    if let Some((first, second)) = tied {
        if !lenient {
            return Err(ambiguity(&lower, first, second, "class"));
        }
        return Ok(json!({"class":"unknown"}).as_object().unwrap().clone());
    }
    let Some((_, class)) = winner else { return Ok(json!({"class":"unknown"}).as_object().unwrap().clone()) };
    let mut out = Map::new();
    out.insert("class".into(), class["id"].clone());
    if let Some(family) = classify_family(class, bare, &lower, lenient)? {
        out.insert("family".into(), family.into());
    }
    if let Some(revision) = extract_revision(class, bare) {
        out.insert("revision".into(), revision.into());
    }
    Ok(out)
}

fn find_identity_override<'a>(
    classes: &'a [Value],
    provider: &str,
    bare: &str,
    observed: Option<f64>,
) -> Option<&'a Value> {
    let lower_provider = provider.to_lowercase();
    let lower_model = bare.to_lowercase();
    let mut agnostic = None;
    for class in classes {
        for rule in array(&class["overrides"]) {
            if string(&rule["model"]).to_lowercase() != lower_model {
                continue;
            }
            if rule
                .get("expiresAtMs")
                .and_then(Value::as_f64)
                .zip(observed)
                .is_some_and(|(expires, now)| now >= expires)
            {
                continue;
            }
            if let Some(provider) = rule.get("provider") {
                if string(provider).to_lowercase() == lower_provider {
                    return Some(rule);
                }
            } else if agnostic.is_none() {
                agnostic = Some(rule);
            }
        }
    }
    agnostic
}

pub fn classify_model(provider: &str, model_id: &str, options: ClassifyOptions) -> Result<Value, CatalogPolicyError> {
    classify_over_taxonomy(&compiled_rules()["taxonomy"], provider, model_id, options)
}
fn classify_over_taxonomy(
    taxonomy: &Value,
    provider: &str,
    model_id: &str,
    options: ClassifyOptions,
) -> Result<Value, CatalogPolicyError> {
    let classes = array(&taxonomy["classes"]);
    let trimmed = ara_prompt::js::trim(model_id);
    if let Some(rule) = find_identity_override(classes, provider, bare_of(trimmed), options.observed_at_ms) {
        let logical = rule.get("logical").and_then(Value::as_str).unwrap_or(trimmed);
        let class = match rule.get("class").and_then(Value::as_str) {
            Some(class) => class.to_owned(),
            None => string(&classify_ranks(classes, logical, options.lenient)?["class"]).to_owned(),
        };
        let inferred = ranks_in_class(classes, &class, logical, options.lenient)?;
        let mut out = Map::new();
        out.insert("class".into(), class.into());
        for name in ["family", "revision"] {
            if let Some(value) = rule.get(name).filter(|v| !v.is_null()).or_else(|| inferred.get(name)) {
                out.insert(name.into(), value.clone());
            }
        }
        if let Some(effort) = rule.get("effort") {
            out.insert("effort".into(), effort.clone());
        }
        if rule.get("thinkingVariant").is_some_and(ara_prompt::js::truthy) {
            out.insert("thinkingVariant".into(), true.into());
        }
        if logical != trimmed {
            out.insert("logicalId".into(), logical.into());
        }
        return Ok(Value::Object(out));
    }
    let collapsed = if trimmed.len() == model_id.len() {
        collapse_over_vocabulary(&taxonomy["collapse"], provider, trimmed)
    } else {
        json!({"logicalId":trimmed,"thinkingVariant":false})
    };
    let logical = string(&collapsed["logicalId"]);
    let mut out = classify_ranks(classes, logical, options.lenient)?;
    if let Some(effort) = collapsed.get("effort") {
        out.insert("effort".into(), effort.clone());
    }
    if collapsed["thinkingVariant"] == true {
        out.insert("thinkingVariant".into(), true.into());
    }
    if logical != trimmed {
        out.insert("logicalId".into(), logical.into());
    }
    Ok(Value::Object(out))
}

pub fn collapse_variant_id(provider: &str, model: &str) -> Value {
    collapse_over_vocabulary(collapse_vocabulary(), provider, model)
}
fn collapse_over_vocabulary(collapse: &Value, provider: &str, model: &str) -> Value {
    let lower = model.to_lowercase();
    let provider = provider.to_lowercase();
    for family in array(&collapse["effortFamilies"]) {
        if family["provider"] == provider && contains(&family["aliases"], &lower) {
            return json!({"logicalId":family["logical"],"thinkingVariant":false});
        }
    }
    let bare = bare_of(&lower);
    let mut winner: Option<&Value> = None;
    for rule in array(&collapse["suffixes"]) {
        let suffix = string(&rule["suffix"]);
        if !lower.ends_with(suffix) || rule.get("exceptBarePrefix").is_some_and(|p| bare.starts_with(string(p))) {
            continue;
        }
        if winner.is_none_or(|w| js_len(suffix) > js_len(string(&w["suffix"]))) {
            winner = Some(rule);
        }
    }
    if let Some(winner) = winner {
        let mut out = json!({"logicalId":js_slice(model,0,js_len(model).saturating_sub(js_len(string(&winner["suffix"])))),"thinkingVariant":winner["thinking"] == true});
        if let Some(effort) = winner.get("effort") {
            out["effort"] = effort.clone();
        }
        return out;
    }
    for lane in array(&collapse["lanes"]) {
        let suffix = string(&lane["suffix"]);
        if !contains(&lane["providers"], &provider) || !lower.ends_with(suffix) {
            continue;
        }
        let trimmed = &lower[..lower.len() - suffix.len()];
        let bare = bare_of(trimmed);
        if lane.get("barePrefix").is_some_and(|p| !bare.starts_with(string(p))) {
            continue;
        }
        let mut effort: Option<&Value> = None;
        for rule in array(&collapse["suffixes"]) {
            let suffix = string(&rule["suffix"]);
            if rule.get("effort").is_none()
                || !trimmed.ends_with(suffix)
                || rule.get("exceptBarePrefix").is_some_and(|p| bare.starts_with(string(p)))
            {
                continue;
            }
            if effort.is_none_or(|w| js_len(suffix) > js_len(string(&w["suffix"]))) {
                effort = Some(rule);
            }
        }
        let Some(effort) = effort else { continue };
        let base = js_slice(model, 0, js_len(trimmed).saturating_sub(js_len(string(&effort["suffix"]))));
        if base.is_empty() || base.ends_with('/') {
            continue;
        }
        return json!({"logicalId":format!("{}{}",base,js_slice(model,js_len(trimmed),js_len(model))),"effort":effort["effort"],"thinkingVariant":false});
    }
    json!({"logicalId":model,"thinkingVariant":false})
}

/// Lossless JavaScript string result, including a lone surrogate produced by
/// applying indices from a case-expanded lowercase string to the original.
/// Presence-only consumers must use this API rather than a UTF-8 projection.
pub fn strip_thinking_variant_suffix_utf16(model: &str) -> Option<Vec<u16>> {
    let lower = model.to_lowercase();
    let bytes = lower.as_bytes();
    for token in array(&collapse_vocabulary()["pairTokens"]) {
        let needle = format!("-{}", string(token));
        let mut from = 0;
        while from < lower.len() {
            let Some(offset) = lower[from..].find(&needle) else { break };
            let index = from + offset;
            let end = index + needle.len();
            let follows = bytes.get(end).is_some_and(u8::is_ascii_alphanumeric);
            let mut start = index;
            while start > 0 && bytes[start - 1].is_ascii_alphanumeric() {
                start -= 1;
            }
            if !follows && !matches!(&lower[start..index], "non" | "no") {
                let original: Vec<_> = model.encode_utf16().collect();
                let mut stripped = original[..js_len(&lower[..index]).min(original.len())].to_vec();
                stripped.extend_from_slice(&original[js_len(&lower[..end]).min(original.len())..]);
                return (!stripped.is_empty()).then_some(stripped);
            }
            from = index + 1;
        }
    }
    None
}

/// Checked UTF-8 projection. JavaScript permits lone UTF-16 surrogates; Rust
/// strings and serde_json::Value do not. Keep that result in the lossless API
/// above, and never replace it with U+FFFD or report it as no matching suffix.
pub fn strip_thinking_variant_suffix(model: &str) -> Result<Option<String>, CatalogPolicyError> {
    strip_thinking_variant_suffix_utf16(model)
        .map(|units| {
            String::from_utf16(&units).map_err(|_| {
                CatalogPolicyError(
                    "thinking variant result contains a lone UTF-16 surrogate; use strip_thinking_variant_suffix_utf16"
                        .into(),
                )
            })
        })
        .transpose()
}

pub fn billing_variant_plain(model: &str) -> Option<String> {
    for suffix in array(&discovery_vocabulary()["billingVariantSuffixes"]) {
        let suffix = string(suffix);
        let length = js_len(model);
        if length <= js_len(suffix) {
            continue;
        }
        let split = length - js_len(suffix);
        if suffix_matches_at(model, split, suffix) {
            return Some(js_slice(model, 0, split));
        }
    }
    None
}
pub fn routing_variant_plain(provider: &str, model: &str) -> Option<String> {
    let provider = provider.to_lowercase();
    for rule in array(&collapse_vocabulary()["routingVariants"]) {
        if !contains(&rule["providers"], &provider) {
            continue;
        }
        let suffix = string(&rule["suffix"]);
        let length = js_len(model);
        if length <= js_len(suffix) {
            continue;
        }
        let split = length - js_len(suffix);
        if suffix_matches_at(model, split, suffix) {
            return Some(js_slice(model, 0, split));
        }
    }
    None
}
pub fn has_routing_variants(provider: &str) -> bool {
    let provider = provider.to_lowercase();
    array(&collapse_vocabulary()["routingVariants"]).iter().any(|r| contains(&r["providers"], &provider))
}
pub fn recovers_canonical_params(provider: &str) -> bool {
    contains(&discovery_vocabulary()["canonicalRecovery"], &provider.to_lowercase())
}
pub fn responses_hint_group(provider: &str) -> Option<&'static Value> {
    let provider = provider.to_lowercase();
    array(&discovery_vocabulary()["responsesHintGroups"]).iter().find(|g| contains(g, &provider))
}
pub fn responses_route_models(provider: &str) -> Option<&'static Value> {
    discovery_vocabulary()["responsesRouteModels"].get(provider.to_lowercase())
}
pub fn supports_dynamic_effort_siblings(provider: &str) -> bool {
    effort_families_for(provider).into_iter().any(|f| !string(&f["logical"]).is_empty())
}
pub fn effort_families_for(provider: &str) -> Vec<&'static Value> {
    let provider = provider.to_lowercase();
    array(&collapse_vocabulary()["effortFamilies"]).iter().filter(|f| f["provider"] == provider).collect()
}
pub fn strip_effort_lane(provider: &str, model: &str) -> String {
    let provider = provider.to_lowercase();
    for lane in array(&collapse_vocabulary()["lanes"]) {
        if !contains(&lane["providers"], &provider) {
            continue;
        }
        let suffix = string(&lane["suffix"]);
        let length = js_len(model);
        if length < js_len(suffix) {
            continue;
        }
        let split = length - js_len(suffix);
        if suffix_matches_at(model, split, suffix) {
            return js_slice(model, 0, split);
        }
    }
    model.into()
}
pub fn collapse_vocabulary() -> &'static Value {
    &compiled_rules()["taxonomy"]["collapse"]
}
pub fn discovery_vocabulary() -> &'static Value {
    &compiled_rules()["taxonomy"]["discovery"]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revisions_literals_prefixes_and_constraints_cover_complete_u8_contract() {
        for (input, expected) in [
            ("4", [4, 0, 0]),
            ("4.6", [4, 6, 0]),
            ("4-6-1", [4, 6, 1]),
            ("0004.06-01", [4, 6, 1]),
            ("255.255.255", [255; 3]),
        ] {
            assert_eq!(parse_revision(input), Some(expected));
            assert_eq!(parse_revision(&format_revision(expected)), Some(expected));
        }
        for invalid in
            ["", " 4", "4 ", "4..2", "4.2.", "4.2.0.0", "256", "1.256", "1.1.256", "-4", "+4", "4a", "４", "0x4"]
        {
            assert_eq!(parse_revision(invalid), None, "{invalid}");
        }
        for (input, expected) in [
            ("4-6-turbo", Some([4, 6, 0])),
            ("3.3-70b", Some([3, 3, 0])),
            ("32b", None),
            ("256.1", None),
            ("4.256", Some([4, 0, 0])),
            ("1.2.3.4", Some([1, 2, 3])),
            ("4é", Some([4, 0, 0])),
            ("4.", Some([4, 0, 0])),
        ] {
            assert_eq!(parse_revision_prefix(input), expected, "{input}");
        }
        let terms = parse_revision_constraint("\u{feff}>=2.5\t<3.8 <=255 >0 =2.5.0").unwrap();
        assert!(revision_satisfies([2, 5, 0], &terms));
        assert!(!revision_satisfies([3, 8, 0], &terms));
        assert!(revision_satisfies([0, 0, 0], &[]));
        for invalid in ["", "2", ">=2-5", ">= 2", "=>2", ">=256", ">=1\u{85}<2"] {
            assert!(parse_revision_constraint(invalid).is_none(), "{invalid}");
        }
        assert_eq!(compare_revision([255, 0, 0], [0, 255, 255]), 255);
        assert_eq!(compare_revision([1, 0, 0], [1, 255, 0]), -255);
    }

    fn target() -> Value {
        json!({"provider":"prov","class":"cls","model":"model-1","reasoning":true})
    }
    #[test]
    fn cascade_specificity_dimensions_priority_and_per_axis_selection() {
        let rules = json!({"rules":[
            {"source":"base:1","class":"cls","wire":{"store":true,"retained":"base"}},
            {"source":"glob:2","class":"cls","models":[{"kind":"glob","value":"model-*"}],"wire":{"store":true}},
            {"source":"exact:3","models":[{"kind":"exact","value":"model-1"}],"wire":{"store":false}},
            {"source":"family:4","class":"cls","family":"fam","catalog":{"limit":20}},
            {"source":"revision:5","class":"cls","revision":[{"op":">=","revision":"2"}],"priority":1,"catalog":{"limit":30}}
        ]});
        let mut t = target();
        t["family"] = "fam".into();
        t["revision"] = "2.5".into();
        assert_eq!(
            resolve_cascade_rules(&rules, &t).unwrap(),
            json!({"wire":{"store":false,"retained":"base"},"thinking":{},"catalog":{"limit":30}})
        );
        t["model"] = "MODEL-1".into();
        assert_eq!(resolve_cascade_rules(&rules, &t).unwrap()["wire"]["store"], true);
        t["class"] = "other".into();
        assert_eq!(resolve_cascade_rules(&rules, &t).unwrap(), json!({"wire":{},"thinking":{},"catalog":{}}));
    }

    #[test]
    fn cascade_ties_report_exact_sources_even_equal_values_and_later_stronger_rule() {
        let rules = json!({"rules":[
            {"source":"first.kdl:8","class":"cls","wire":{"x":false}},
            {"source":"second.kdl:9","class":"cls","wire":{"x":false}},
            {"source":"later.kdl:10","class":"cls","priority":9,"wire":{"x":true}}
        ]});
        assert_eq!(
            resolve_cascade_rules(&rules, &target()).unwrap_err().to_string(),
            "ambiguous overlap for `prov/model-1` on axis `x`: rules `first.kdl:8` and `second.kdl:9` tie; add an explicit priority"
        );
        let invalid = json!({"rules":[{"source":"bad.kdl:12","revision":[{"op":">=","revision":"256"}]}]});
        assert_eq!(
            resolve_cascade_rules(&invalid, &target()).unwrap_err().to_string(),
            "invalid compiled revision term in bad.kdl:12"
        );
    }

    #[test]
    fn cascade_reasoning_upgrade_requires_matching_exact_efforts_and_empty_constraints_do_not_match() {
        let rules = json!({"rules":[
            {"source":"b:1","thinking":{"mode":"effort"}},
            {"source":"e:2","providers":["prov"],"models":[{"kind":"exact","value":"model-1"}],"thinking":{"efforts":["low","high"]}},
            {"source":"token:3","models":[{"kind":"token","value":"model"}],"wire":{"token":true}},
            {"source":"empty:4","models":[],"wire":{"never":true}},
            {"source":"provider:5","providers":[],"wire":{"never":true}},
            {"source":"rev:6","revision":[],"catalog":{"requiresRevision":true}}
        ]});
        let mut t = target();
        t["reasoning"] = false.into();
        assert_eq!(
            resolve_cascade_rules(&rules, &t).unwrap(),
            json!({"wire":{"token":true},"thinking":{"mode":"effort","efforts":["low","high"]},"catalog":{}})
        );
        t["provider"] = "PROV".into();
        t["revision"] = "1".into();
        assert_eq!(
            resolve_cascade_rules(&rules, &t).unwrap(),
            json!({"wire":{"token":true},"thinking":{},"catalog":{"requiresRevision":true}})
        );
        t["revision"] = "bad".into();
        assert_eq!(resolve_cascade_rules(&rules, &t).unwrap()["catalog"], json!({}));
    }

    #[test]
    fn glob_is_anchored_and_does_not_reuse_consumed_segments() {
        for (pattern, value, expected) in [
            ("gpt-*-codex", "gpt-5.2-codex", true),
            ("gpt-*-codex", "xgpt-5.2-codex", false),
            ("*sonnet*", "claude-sonnet-4-5", true),
            ("a*a", "a", false),
            ("a**b*c", "abxc", true),
            ("*", "", true),
            ("A*", "a", false),
            ("*😀*", "x😀z", true),
        ] {
            assert_eq!(glob_match(pattern, value), expected, "{pattern} {value}");
        }
    }

    #[test]
    fn fixed_taxonomy_source_fixtures_and_trim_bypass() {
        let cases = [
            ("anthropic", "claude-opus-4-6", json!({"class":"anthropic","family":"opus","revision":"4.6.0"})),
            (
                "openrouter",
                "anthropic/claude-opus-4-6",
                json!({"class":"anthropic","family":"opus","revision":"4.6.0"}),
            ),
            ("litellm", "mistral.mixtral-8x7b-instruct-v0:1", json!({"class":"mistral","family":"mixtral"})),
            ("litellm", "mistral.mistral-large-2402-v1:0", json!({"class":"mistral","family":"mistral"})),
            ("openai", "o3", json!({"class":"openai","family":"o-series"})),
            ("openai", "o3-mini", json!({"class":"openai","family":"o-series","revision":"3.0.0"})),
            ("cerebras", "zai-glm-4.7", json!({"class":"glm","revision":"4.7.0"})),
            ("test", "anthropicology", json!({"class":"unknown"})),
            ("test", "deepseeker", json!({"class":"unknown"})),
        ];
        for (provider, model, expected) in cases {
            assert_eq!(classify_model(provider, model, Default::default()).unwrap(), expected, "{model}");
        }
        let qwen = classify_model("alibaba-token-plan", "qwen3.6-max", Default::default()).unwrap();
        assert_eq!(qwen["class"], "qwen");
        assert!(qwen.get("effort").is_none());
        let daybreak = classify_model("openai", "daybreak-blue-latest", Default::default()).unwrap();
        assert_eq!(daybreak["class"], "unknown");
        assert_eq!(daybreak["revision"], "5.6.0");
        let kilo = classify_model("kilo", "qwq-32b", Default::default()).unwrap();
        assert_eq!(kilo["logicalId"], "qwen/qwq-32b");
        let normal = classify_model("test", "glm-4.6-thinking", Default::default()).unwrap();
        assert_eq!(normal["logicalId"], "glm-4.6");
        assert_eq!(normal["thinkingVariant"], true);
        let whitespace = classify_model("test", " glm-4.6-thinking ", Default::default()).unwrap();
        assert!(whitespace.get("logicalId").is_none());
        assert!(whitespace.get("thinkingVariant").is_none());
    }

    fn class(id: &str) -> Value {
        json!({"id":id,"matchers":[{"kind":"bounded","token":"model"}],"families":[],"revisionPrefixes":[],"skipBare":[],"overrides":[]})
    }
    fn taxonomy(classes: Vec<Value>) -> Value {
        json!({"classes":classes,"collapse":collapse_vocabulary()})
    }
    #[test]
    fn taxonomy_ambiguity_leniency_and_higher_rank_reset() {
        let first = class("one");
        let mut second = class("two");
        let tree = taxonomy(vec![first.clone(), second.clone()]);
        assert_eq!(
            classify_over_taxonomy(&tree, "p", "MODEL-1", Default::default()).unwrap_err().to_string(),
            "ambiguous class for `model-1`: `one` and `two` tie"
        );
        assert_eq!(
            classify_over_taxonomy(&tree, "p", "MODEL-1", ClassifyOptions { lenient: true, ..Default::default() })
                .unwrap(),
            json!({"class":"unknown"})
        );
        second["matchers"].as_array_mut().unwrap().push(json!({"kind":"exact","token":"model-1"}));
        second["families"] = json!([{"id":"a","glob":"*model*","priority":0},{"id":"b","glob":"*model*","priority":0}]);
        let tree = taxonomy(vec![first, second]);
        assert_eq!(
            classify_over_taxonomy(&tree, "p", "MODEL-1", Default::default()).unwrap_err().to_string(),
            "ambiguous family for `model-1`: `a` and `b` tie"
        );
        assert_eq!(
            classify_over_taxonomy(&tree, "p", "MODEL-1", ClassifyOptions { lenient: true, ..Default::default() })
                .unwrap(),
            json!({"class":"two"})
        );
    }

    #[test]
    fn overrides_specific_provider_expiry_and_inference_are_preserved() {
        let mut c = class("one");
        c["revisionPrefixes"] = json!([{"prefix":"model"}]);
        c["overrides"] = json!([
            {"model":"alias","class":"one","logical":"model-2.3","family":"agnostic"},
            {"model":"ALIAS","provider":"P","class":"one","logical":"model-4","effort":"off","thinkingVariant":true,"expiresAtMs":100}
        ]);
        let tree = taxonomy(vec![c]);
        let active = classify_over_taxonomy(&tree, "p", "ns/alias", Default::default()).unwrap();
        assert_eq!(
            active,
            json!({"class":"one","logicalId":"model-4","revision":"4.0.0","effort":"off","thinkingVariant":true})
        );
        let expired = classify_over_taxonomy(
            &tree,
            "p",
            "ns/alias",
            ClassifyOptions { observed_at_ms: Some(100.0), lenient: false },
        )
        .unwrap();
        assert_eq!(expired, json!({"class":"one","logicalId":"model-2.3","revision":"2.3.0","family":"agnostic"}));
        assert_eq!(
            classify_over_taxonomy(
                &tree,
                "p",
                "alias",
                ClassifyOptions { observed_at_ms: Some(f64::NAN), lenient: false }
            )
            .unwrap(),
            active
        );
    }

    #[test]
    fn suffixes_lanes_negations_and_all_discovery_accessors() {
        assert_eq!(
            collapse_variant_id("openai", "gpt-5.2-codex-xhigh"),
            json!({"logicalId":"gpt-5.2-codex","effort":"xhigh","thinkingVariant":false})
        );
        assert_eq!(
            collapse_variant_id("CURSOR", "GPT-5.6-HIGH-FAST"),
            json!({"logicalId":"GPT-5.6-FAST","effort":"high","thinkingVariant":false})
        );
        assert_eq!(
            collapse_variant_id("other", "GPT-5.6-HIGH-FAST"),
            json!({"logicalId":"GPT-5.6-HIGH-FAST","thinkingVariant":false})
        );
        for (input, expected) in [
            ("abc-thinking-v2", Some("abc-v2")),
            ("abc-reasoning", Some("abc")),
            ("abc-reasoner", Some("abc")),
            ("abc-non-thinking", None),
            ("abc-no-thinking", None),
            ("abc-thinking2", None),
            ("-thinking", None),
            ("a-no-thinking-thinking", Some("a-no-thinking")),
            ("😀-thinking", Some("😀")),
        ] {
            assert_eq!(strip_thinking_variant_suffix(input).unwrap().as_deref(), expected, "{input}");
        }
        assert_eq!(strip_thinking_variant_suffix_utf16("İ-thinking😀"), Some(vec![0x0130, 0x002d, 0xde00]));
        assert!(strip_thinking_variant_suffix("İ-thinking😀").is_err());
        assert_eq!(routing_variant_plain("OPENAI-CODEX", "gpt-5.6-luna-WM").as_deref(), Some("gpt-5.6-luna"));
        assert_eq!(routing_variant_plain("openai", "gpt-5.6-luna-wm"), None);
        assert_eq!(billing_variant_plain("gpt-5.5-pro-free").as_deref(), Some("gpt-5.5-pro"));
        assert_eq!(billing_variant_plain("-free"), None);
        assert_eq!(billing_variant_plain("😀xxxx"), None);
        assert_eq!(routing_variant_plain("openai-codex", "😀xx"), None);
        assert_eq!(strip_effort_lane("cursor", "😀xxxx"), "😀xxxx");
        assert_eq!(strip_effort_lane("CURSOR", "model-FAST"), "model");
        assert_eq!(strip_effort_lane("cursor", "-fast"), "");
        assert!(has_routing_variants("OPENAI-CODEX"));
        assert!(!has_routing_variants("missing"));
        assert!(supports_dynamic_effort_siblings("CURSOR"));
        assert!(!supports_dynamic_effort_siblings("missing"));
        assert_eq!(effort_families_for("CURSOR").len(), 3);
        for provider in array(&discovery_vocabulary()["canonicalRecovery"]) {
            assert!(recovers_canonical_params(&string(provider).to_uppercase()));
        }
        for group in array(&discovery_vocabulary()["responsesHintGroups"]) {
            for provider in array(group) {
                assert_eq!(responses_hint_group(&string(provider).to_uppercase()), Some(group));
            }
        }
        for (provider, models) in discovery_vocabulary()["responsesRouteModels"].as_object().unwrap() {
            assert_eq!(responses_route_models(&provider.to_uppercase()), Some(models));
        }
        assert_eq!(responses_hint_group("missing"), None);
        assert_eq!(responses_route_models("missing"), None);
        assert_eq!(RULES_JSON.len(), 321_052);
        assert_eq!(array(&compiled_rules()["taxonomy"]["classes"]).len(), 21);
        assert_eq!(array(&compiled_rules()["cascade"]["rules"]).len(), 563);
        assert!(std::ptr::eq(compiled_rules(), compiled_rules()));
    }

    #[test]
    fn fixed_glm_revision_ladder_and_all_bundled_targets_resolve_without_ties() {
        let result = resolve_cascade(&json!({"provider":"alibaba-coding-plan","class":"glm","model":"glm-5.2","revision":"5.2","reasoning":true})).unwrap();
        assert_eq!(result["thinking"]["efforts"], json!(["minimal", "low", "medium", "high", "max"]));
        let catalog: Value = serde_json::from_slice(crate::model_catalog::BUNDLED_JSON).unwrap();
        let mut count = 0;
        for (provider, models) in catalog.as_object().unwrap() {
            for (id, model) in models.as_object().unwrap() {
                let identity = classify_model(provider, id, Default::default()).unwrap();
                let mut target = identity.as_object().unwrap().clone();
                target.insert("provider".into(), provider.clone().into());
                target.insert("model".into(), id.clone().into());
                target.insert("reasoning".into(), model["reasoning"].clone());
                resolve_cascade(&Value::Object(target)).unwrap_or_else(|e| panic!("{provider}/{id}: {e}"));
                count += 1;
            }
        }
        assert_eq!(count, 4776);
    }
}
