//! Complete Host-owned variant collapsing from fixed OMP 596f2da.
//!
//! Source: packages/catalog/src/compat/collapse.ts. A runtime is shared by all
//! catalog consumers in a Host: alias registration and lazy template learning
//! persist across calls. Model references retain object identity with `Arc`.
//
// MIT License
// Copyright (c) 2025 Mario Zechner
// Copyright (c) 2025-2026 Can Bölük
// Copyright (c) 2026 Stencil Labs, Inc.
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::{Arc, Mutex},
};

use crate::{
    catalog_rules::{self, RevisionTerm},
    js_regex::JsRegExp,
    model_policy,
};
use ara_rpc::{WireString, WireValue};

const EFFORTS: &[&str] = &["minimal", "low", "medium", "high", "xhigh", "max"];
const ROUTE_KEYS: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh", "max"];
const DEFAULT_PAIR_EFFORTS: &[&str] = &["minimal", "low", "medium", "high"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollapseError {
    message: String,
    wire_message: Option<WireString>,
}
impl CollapseError {
    pub fn new(message: String) -> Self {
        Self { message, wire_message: None }
    }
    pub fn from_wire(message: WireString) -> Self {
        let display = message.to_utf8().unwrap_or_else(|_| String::from_utf16_lossy(message.units()));
        Self { message: display, wire_message: Some(message) }
    }
    /// Error transport keeps the source UTF-16 message. Display is for local
    /// diagnostics and never substitutes for this value in a receipt.
    pub fn message_wire(&self) -> WireString {
        self.wire_message.clone().unwrap_or_else(|| WireString::from(self.message.as_str()))
    }
    pub fn name(&self) -> &'static str {
        if self.message.starts_with("undefined is not an object (evaluating '") {
            "TypeError"
        } else if self.message.starts_with("ambiguous class for ") || self.message.starts_with("ambiguous family for ")
        {
            "AmbiguousIdentityError"
        } else {
            "Error"
        }
    }
}
impl fmt::Display for CollapseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for CollapseError {}

/// An open model record. Undefined own properties retain their original key
/// position as null placeholders; only this sidecar gives them undefined
/// semantics. Actual authored null is never an undefined placeholder.
#[derive(Debug, Clone)]
pub struct VariantSpec {
    pub value: WireValue,
    pub undefined_paths: Vec<Vec<WireString>>,
}
pub type SpecRef = Arc<VariantSpec>;
pub type EffortVariantFamily = VariantSpec;

impl VariantSpec {
    pub fn from_wire(value: WireValue) -> Self {
        Self { value, undefined_paths: Vec::new() }
    }
    pub fn from_json(value: &serde_json::Value) -> Self {
        Self::from_wire(WireValue::parse(&value.to_string()).expect("serde JSON is wire JSON"))
    }
    pub fn get(&self, key: &str) -> Option<&WireValue> {
        self.get_path(&[WireString::from(key)])
    }
    pub fn get_path(&self, path: &[WireString]) -> Option<&WireValue> {
        if self.undefined_paths.iter().any(|undefined| undefined == path) {
            return None;
        }
        raw_path(&self.value, path)
    }
    pub fn set(&mut self, key: &str, value: WireValue) {
        self.clear_undefined(key);
        self.value.insert(key, value);
    }
    pub fn set_undefined(&mut self, key: &str) {
        self.clear_undefined(key);
        self.value.insert(key, WireValue::Null);
        self.undefined_paths.push(vec![key.into()]);
    }
    pub fn remove(&mut self, key: &str) {
        self.clear_undefined(key);
        if let WireValue::Object(entries) = &mut self.value {
            entries.retain(|(candidate, _)| !candidate.equals_ascii(key));
        }
    }
    fn clear_undefined(&mut self, key: &str) {
        self.undefined_paths.retain(|path| !path.first().is_some_and(|part| part.equals_ascii(key)));
    }
    pub(crate) fn set_record(&mut self, key: &str, record: &Self) {
        self.set(key, record.value.clone());
        self.undefined_paths.extend(record.undefined_paths.iter().map(|path| {
            let mut next = vec![WireString::from(key)];
            next.extend(path.iter().cloned());
            next
        }));
    }
    pub(crate) fn record(&self, key: &str) -> Option<Self> {
        self.record_key(&WireString::from(key))
    }
    pub(crate) fn record_key(&self, key: &WireString) -> Option<Self> {
        let value = self.get_path(std::slice::from_ref(key))?.clone();
        let undefined_paths = self
            .undefined_paths
            .iter()
            .filter(|path| path.first() == Some(key) && path.len() > 1)
            .map(|path| path[1..].to_vec())
            .collect();
        Some(Self { value, undefined_paths })
    }
    pub(crate) fn set_key_record(&mut self, key: &WireString, record: &Self) {
        self.undefined_paths.retain(|path| path.first() != Some(key));
        if let WireValue::Object(entries) = &mut self.value {
            if let Some((_, value)) = entries.iter_mut().find(|(held, _)| held == key) {
                *value = record.value.clone();
            } else {
                entries.push((key.clone(), record.value.clone()));
            }
        }
        self.undefined_paths.extend(record.undefined_paths.iter().map(|path| {
            let mut prefixed = vec![key.clone()];
            prefixed.extend(path.iter().cloned());
            prefixed
        }));
    }
    pub(crate) fn set_key_undefined(&mut self, key: &WireString) {
        self.set_key_record(key, &Self::from_wire(WireValue::Null));
        self.undefined_paths.push(vec![key.clone()]);
    }
    /// JSON.stringify-compatible projection; own undefined properties are
    /// omitted in objects and emitted as null in arrays.
    pub fn to_wire_json(&self) -> WireValue {
        project_defined(&self.value, &[], &self.undefined_paths)
    }
    pub fn own_keys(&self) -> Vec<WireString> {
        self.value.entries().unwrap_or_default().iter().map(|(key, _)| (*key).clone()).collect()
    }
}

fn raw_path<'a>(value: &'a WireValue, path: &[WireString]) -> Option<&'a WireValue> {
    let mut current = value;
    for part in path {
        current = match current {
            WireValue::Object(entries) => &entries.iter().find(|(key, _)| key == part)?.1,
            WireValue::Array(items) => items.get(part.to_utf8().ok()?.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}
fn project_defined(value: &WireValue, path: &[WireString], undefined: &[Vec<WireString>]) -> WireValue {
    match value {
        WireValue::Object(_) => WireValue::Object(
            value
                .entries()
                .unwrap()
                .into_iter()
                .filter_map(|(key, item)| {
                    let mut next = path.to_vec();
                    next.push(key.clone());
                    (!undefined.contains(&next)).then(|| (key.clone(), project_defined(item, &next, undefined)))
                })
                .collect(),
        ),
        WireValue::Array(items) => WireValue::Array(
            items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let mut next = path.to_vec();
                    next.push(index.to_string().into());
                    if undefined.contains(&next) { WireValue::Null } else { project_defined(item, &next, undefined) }
                })
                .collect(),
        ),
        _ => value.clone(),
    }
}
pub(crate) fn truthy(value: &WireValue) -> bool {
    match value {
        WireValue::Null => false,
        WireValue::Bool(value) => *value,
        WireValue::Number(value) => *value != 0.0 && !value.is_nan(),
        WireValue::String(value) => !value.is_empty(),
        WireValue::Array(_) | WireValue::Object(_) => true,
    }
}
fn text(spec: &VariantSpec, key: &str) -> Result<WireString, CollapseError> {
    spec.get(key)
        .and_then(WireValue::as_string)
        .cloned()
        .ok_or_else(|| CollapseError::new(format!("model {key} must be a string")))
}
fn strings(value: Option<&WireValue>) -> Vec<WireString> {
    value
        .and_then(WireValue::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|item| item.as_string().cloned())
        .collect()
}
fn property<'a>(value: Option<&'a WireValue>, key: &str) -> Option<&'a WireValue> {
    value?.get(key)
}
fn object() -> WireValue {
    WireValue::Object(Vec::new())
}
fn value_string(value: WireString) -> WireValue {
    WireValue::String(value)
}
fn ascii_string(value: &str) -> WireValue {
    value_string(value.into())
}
fn string_array(values: &[WireString]) -> WireValue {
    WireValue::Array(values.iter().cloned().map(value_string).collect())
}
fn ascii_array(values: &[&str]) -> WireValue {
    WireValue::Array(values.iter().map(|value| ascii_string(value)).collect())
}
fn is_nullish(value: Option<&WireValue>) -> bool {
    value.is_none_or(|value| matches!(value, WireValue::Null))
}
fn strict_equal(a: Option<&WireValue>, b: Option<&WireValue>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(WireValue::Number(a)), Some(WireValue::Number(b))) => a == b,
        (Some(a), Some(b))
            if a.is_object()
                || b.is_object()
                || matches!(a, WireValue::Array(_))
                || matches!(b, WireValue::Array(_)) =>
        {
            std::ptr::eq(a, b)
        }
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}
fn deep_equal_at(a: &VariantSpec, b: &VariantSpec, path: &[WireString]) -> bool {
    match (a.get_path(path), b.get_path(path)) {
        (None, None) => true,
        (None, _) | (_, None) => false,
        (Some(WireValue::Object(_)), Some(WireValue::Object(_))) => {
            let left = a.get_path(path).unwrap().entries().unwrap();
            let right = b.get_path(path).unwrap().entries().unwrap();
            let mut keys = Vec::new();
            for (key, _) in left.into_iter().chain(right) {
                if !keys.contains(key) {
                    keys.push(key.clone());
                }
            }
            keys.into_iter().all(|key| {
                let mut next = path.to_vec();
                next.push(key);
                deep_equal_at(a, b, &next)
            })
        }
        (Some(WireValue::Array(a_items)), Some(WireValue::Array(b_items))) => {
            a_items.len() == b_items.len()
                && (0..a_items.len()).all(|index| {
                    let mut next = path.to_vec();
                    next.push(index.to_string().into());
                    deep_equal_at(a, b, &next)
                })
        }
        (Some(a), Some(b)) => a.deep_equal(b),
    }
}
pub(crate) fn lower(value: &WireString) -> WireString {
    let mut out = Vec::new();
    let mut valid = String::new();
    for item in char::decode_utf16(value.units().iter().copied()) {
        match item {
            Ok(ch) => valid.push(ch),
            Err(error) => {
                out.extend(valid.to_lowercase().encode_utf16());
                valid.clear();
                out.push(error.unpaired_surrogate());
            }
        }
    }
    out.extend(valid.to_lowercase().encode_utf16());
    WireString::from_units(out)
}
fn js_space(unit: u16) -> bool {
    matches!(unit, 0x0009..=0x000d | 0x0020 | 0x00a0 | 0x1680 | 0x2000..=0x200a | 0x2028 | 0x2029 | 0x202f | 0x205f | 0x3000 | 0xfeff)
}
pub(crate) fn trim(value: &WireString) -> WireString {
    let units = value.units();
    let start = units.iter().position(|unit| !js_space(*unit)).unwrap_or(units.len());
    let end = units.iter().rposition(|unit| !js_space(*unit)).map_or(start, |index| index + 1);
    WireString::from_units(units[start..end].to_vec())
}
fn concat(left: &WireString, right: &WireString) -> WireString {
    let mut units = left.units().to_vec();
    units.extend(right.units());
    WireString::from_units(units)
}
fn append(value: &WireString, suffix: &str) -> WireString {
    let mut value = value.clone();
    value.append_str(suffix);
    value
}
fn replace_all(value: &WireString, from: &str, to: &WireString) -> WireString {
    let needle: Vec<u16> = from.encode_utf16().collect();
    let mut out = Vec::new();
    let mut index = 0;
    while index < value.len() {
        if value.units()[index..].starts_with(&needle) {
            out.extend(to.units());
            index += needle.len();
        } else {
            out.push(value.units()[index]);
            index += 1;
        }
    }
    WireString::from_units(out)
}
fn make_regex(source: WireString, flags: &str) -> Result<JsRegExp, CollapseError> {
    JsRegExp::new(source, flags).map_err(|error| CollapseError::new(error.to_string()))
}

pub struct VariantFamilyTemplate {
    pub family: EffortVariantFamily,
    pub revision: Option<Vec<RevisionTerm>>,
    pub pattern: Arc<Mutex<JsRegExp>>,
}
pub struct VariantCollapseTable {
    pub families: Vec<EffortVariantFamily>,
    pub templates: Option<Vec<VariantFamilyTemplate>>,
    pub provider_aliases: Option<WireValue>,
}
impl VariantCollapseTable {
    pub fn new(families: Vec<EffortVariantFamily>) -> Self {
        Self { families, templates: None, provider_aliases: None }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BareVariantAliasHit {
    pub id: WireString,
    pub providers: Vec<WireString>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InheritedTableProperty {
    ObjectPrototype,
    ObjectConstructor,
    Function(&'static str),
}
pub enum ReviewedCollapseLookup {
    Table(Arc<VariantCollapseTable>),
    InheritedProperty(InheritedTableProperty),
    None,
}
fn inherited_table_property(key: &WireString) -> Option<InheritedTableProperty> {
    if key.equals_ascii("__proto__") {
        return Some(InheritedTableProperty::ObjectPrototype);
    }
    if key.equals_ascii("constructor") {
        return Some(InheritedTableProperty::ObjectConstructor);
    }
    [
        "__defineGetter__",
        "__defineSetter__",
        "hasOwnProperty",
        "__lookupGetter__",
        "__lookupSetter__",
        "isPrototypeOf",
        "propertyIsEnumerable",
        "toLocaleString",
        "toString",
        "valueOf",
    ]
    .into_iter()
    .find(|candidate| key.equals_ascii(candidate))
    .map(InheritedTableProperty::Function)
}

/// A single-index reverse lookup is the original live cached array. Combining
/// static and dynamic sources creates a new deduplicated snapshot, as upstream.
#[derive(Clone)]
pub enum VariantAliasSources {
    Shared(Arc<Mutex<Vec<WireString>>>),
    Snapshot(Arc<Mutex<Vec<WireString>>>),
}
impl VariantAliasSources {
    pub fn snapshot(&self) -> Vec<WireString> {
        match self {
            Self::Shared(values) | Self::Snapshot(values) => {
                values.lock().unwrap_or_else(|error| error.into_inner()).clone()
            }
        }
    }
    pub fn shares_storage(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Shared(a) | Self::Snapshot(a), Self::Shared(b) | Self::Snapshot(b)) => Arc::ptr_eq(a, b),
        }
    }
}

/// Lossless policy boundary. Hosts may bind the existing JSON policy where its
/// fields are representable and a native wire policy for other model records.
pub trait CollapseModelPolicy: Send + Sync {
    fn resolve(&self, spec: &VariantSpec) -> Result<VariantSpec, CollapseError>;
    fn build(&self, spec: &VariantSpec) -> Result<VariantSpec, CollapseError>;
}
pub struct JsonModelPolicy;
impl CollapseModelPolicy for JsonModelPolicy {
    fn resolve(&self, spec: &VariantSpec) -> Result<VariantSpec, CollapseError> {
        let value = policy_json(spec, false)?;
        model_policy::resolve_model_policy(&value)
            .map(|value| VariantSpec::from_json(&value))
            .map_err(|error| CollapseError::new(error.to_string()))
    }
    fn build(&self, spec: &VariantSpec) -> Result<VariantSpec, CollapseError> {
        let input = policy_json(spec, true)?;
        let built = model_policy::build_model(&input).map_err(|error| CollapseError::new(error.to_string()))?;
        let mut result = spec.clone();
        built.as_object().ok_or_else(|| CollapseError::new("built model must be object".into()))?;
        // Object spread keeps each authored key in place. New constructor
        // fields enter in build.ts order, including own undefined properties.
        for key in [
            "name",
            "identity",
            "requiresGlyphTokenization",
            "tokenizer",
            "thinking",
            "supportsComputerUse",
            "supportsComputerUseConfig",
            "compat",
        ] {
            let Some(value) = built.get(key) else {
                result.set_undefined(key);
                continue;
            };
            // The JSON engine does not observe opaque metadata or undefined
            // own children. Preserve the source field when its defined view
            // passed through unchanged.
            if input.get(key) == Some(value) {
                continue;
            }
            result.set(key, VariantSpec::from_json(value).value);
        }
        // compatConfig is the authored compat value, rather than its resolved
        // JSON projection, and retains all nested own undefined properties.
        if let Some(compat) = spec.record("compat") {
            result.set_record("compatConfig", &compat);
        } else {
            result.set_undefined("compatConfig");
        }
        // Catalog assignments/corrections run after construction in source
        // order. The projection field is deleted unconditionally when the
        // catalog did not positively select it, including own undefined.
        for key in [
            "serviceTierCost",
            "priority",
            "applyPatchToolType",
            "requiresCursorToolSchemaProjection",
            "contextPromotionTarget",
            "cost",
            "contextWindow",
            "maxTokens",
            "input",
        ] {
            if let Some(value) = built.get(key) {
                if input.get(key) != Some(value) {
                    result.set(key, VariantSpec::from_json(value).value);
                }
            } else if key == "requiresCursorToolSchemaProjection" {
                result.remove(key);
            }
        }
        Ok(result)
    }
}
fn json_representable(value: &WireValue) -> bool {
    match value {
        WireValue::Number(value) => value.is_finite(),
        WireValue::String(value) => value.to_utf8().is_ok(),
        WireValue::Array(items) => items.iter().all(json_representable),
        WireValue::Object(entries) => {
            entries.iter().all(|(key, value)| key.to_utf8().is_ok() && json_representable(value))
        }
        _ => true,
    }
}
fn policy_json(spec: &VariantSpec, build: bool) -> Result<serde_json::Value, CollapseError> {
    let defined = spec.to_wire_json();
    let mut projected = serde_json::Map::new();
    let required = ["id", "provider", "api", "baseUrl", "reasoning", "compat", "thinking", "requestModelId"];
    let build_required = [
        "name",
        "tokenizer",
        "supportsComputerUse",
        "supportsComputerUseConfig",
        "cost",
        "contextWindow",
        "maxTokens",
        "premiumMultiplier",
        "serviceTierCost",
    ];
    for (key, value) in defined.entries().ok_or_else(|| CollapseError::new("model spec must be an object".into()))? {
        let key_utf8 = key.to_utf8();
        if let Ok(key_utf8) = &key_utf8
            && json_representable(value)
        {
            projected.insert(
                key_utf8.clone(),
                serde_json::from_str(&value.stringify()).map_err(|error| CollapseError::new(error.to_string()))?,
            );
        } else if required
            .iter()
            .chain(if build { build_required.as_slice() } else { &[] })
            .any(|required| key.equals_ascii(required))
        {
            return Err(CollapseError::new(format!(
                "lossless policy binding required for {}",
                key_utf8.unwrap_or_else(|_| "UTF-16 key".into())
            )));
        }
        // Unknown metadata is opaque to the policy; it remains in `spec` and
        // is preserved by the build merge rather than coerced into JSON.
    }
    // explicitComputerUseConfig checks own presence before narrowing the
    // value to boolean. A null projection has the same narrowing result as
    // undefined and prevents the source fallback to supportsComputerUse.
    if build
        && spec.undefined_paths.iter().any(|path| path.len() == 1 && path[0].equals_ascii("supportsComputerUseConfig"))
    {
        projected.insert("supportsComputerUseConfig".into(), serde_json::Value::Null);
    }
    Ok(serde_json::Value::Object(projected))
}

fn compiled_family(compiled: &serde_json::Value) -> EffortVariantFamily {
    let mut family = VariantSpec::from_wire(object());
    for key in ["id", "name", "members", "routing"] {
        family.set(key, VariantSpec::from_json(&compiled[key]).value);
    }
    for key in ["defaultMember"] {
        if let Some(value) = compiled.get(key) {
            family.set(key, VariantSpec::from_json(value).value);
        }
    }
    for key in ["retiredMembers"] {
        if compiled.get(key).and_then(serde_json::Value::as_array).is_some_and(|items| !items.is_empty()) {
            family.set(key, VariantSpec::from_json(&compiled[key]).value);
        }
    }
    if compiled["noThinking"] != true && compiled.get("mode").is_some() {
        let mut thinking = object();
        thinking.insert("mode", VariantSpec::from_json(&compiled["mode"]).value);
        thinking
            .insert("efforts", VariantSpec::from_json(compiled.get("efforts").unwrap_or(&serde_json::json!([]))).value);
        for key in ["effortBudgets", "defaultLevel"] {
            if let Some(value) = compiled.get(key) {
                thinking.insert(key, VariantSpec::from_json(value).value);
            }
        }
        if compiled["requiresEffort"] == true {
            thinking.insert("requiresEffort", WireValue::Bool(true));
        }
        family.set("thinking", thinking);
    }
    for key in ["suppressWhenOff", "preserveAbsentEffortRoutes"] {
        if compiled.get(key).is_some_and(ara_prompt::js::truthy) {
            family.set(key, WireValue::Bool(true));
        }
    }
    if compiled.get("extraAliases").and_then(serde_json::Value::as_array).is_some_and(|items| !items.is_empty()) {
        family.set("extraAliases", VariantSpec::from_json(&compiled["extraAliases"]).value);
    }
    family
}
fn escaped_pattern(id: &WireString) -> WireString {
    let placeholder: Vec<u16> = "{rev}".encode_utf16().collect();
    let mut out = Vec::new();
    let mut index = 0;
    while index < id.len() {
        if id.units()[index..].starts_with(&placeholder) {
            out.extend("(\\d+(?:\\.\\d+){0,2})".encode_utf16());
            index += placeholder.len();
        } else {
            let unit = id.units()[index];
            if ".*+?^${}()|[]\\".encode_utf16().any(|special| special == unit) {
                out.push(b'\\' as u16);
            }
            out.push(unit);
            index += 1;
        }
    }
    WireString::from_units(out)
}
fn compiled_template(compiled: &serde_json::Value) -> Result<VariantFamilyTemplate, CollapseError> {
    let family = compiled_family(compiled);
    let mut alternatives = vec![text(&family, "id")?];
    alternatives.extend(strings(family.get("members")));
    alternatives.extend(strings(family.get("extraAliases")));
    let mut pattern = WireString::from("^(?:");
    for (index, alternative) in alternatives.iter().enumerate() {
        if index != 0 {
            pattern.append_str("|");
        }
        pattern = concat(&pattern, &escaped_pattern(alternative));
    }
    pattern.append_str(")$");
    let revision =
        compiled.get("revision").and_then(serde_json::Value::as_str).and_then(catalog_rules::parse_revision_constraint);
    Ok(VariantFamilyTemplate { family, revision, pattern: Arc::new(Mutex::new(make_regex(pattern, "i")?)) })
}
fn template_family_for(
    templates: &[VariantFamilyTemplate],
    id: &WireString,
) -> Result<Option<EffortVariantFamily>, CollapseError> {
    for template in templates {
        let hit =
            template.pattern.lock().map_err(|_| CollapseError::new("template regexp lock poisoned".into()))?.exec(id);
        let Some(hit) = hit else { continue };
        let Some(rev) = hit.captures.iter().skip(1).find_map(Option::as_ref) else { continue };
        if let Some(terms) = &template.revision {
            let Some(parsed) = rev.to_utf8().ok().as_deref().and_then(catalog_rules::parse_revision) else { continue };
            if !catalog_rules::revision_satisfies(parsed, terms) {
                continue;
            }
        }
        let mut family = template.family.clone();
        for key in ["id", "name", "defaultMember"] {
            if let Some(value) = family.get(key).and_then(WireValue::as_string).cloned() {
                family.set(key, value_string(replace_all(&value, "{rev}", rev)));
            }
        }
        for key in ["members", "retiredMembers", "extraAliases"] {
            if family.get(key).is_some() {
                family.set(
                    key,
                    string_array(
                        &strings(family.get(key))
                            .iter()
                            .map(|value| replace_all(value, "{rev}", rev))
                            .collect::<Vec<_>>(),
                    ),
                );
            }
        }
        let mut routing = object();
        for key in ROUTE_KEYS {
            if let Some(target) = property(family.get("routing"), key).and_then(WireValue::as_string) {
                routing.insert(key, value_string(replace_all(target, "{rev}", rev)));
            }
        }
        family.set("routing", routing);
        return Ok(Some(family));
    }
    Ok(None)
}
fn instantiate_templates(
    table: &VariantCollapseTable,
    ids: &[WireString],
) -> Result<Vec<EffortVariantFamily>, CollapseError> {
    let Some(templates) = &table.templates else { return Ok(Vec::new()) };
    let mut seen: HashSet<WireString> =
        table.families.iter().map(|family| text(family, "id")).collect::<Result<_, _>>()?;
    let mut out = Vec::new();
    for id in ids {
        if let Some(family) = template_family_for(templates, id)?
            && seen.insert(text(&family, "id")?)
        {
            out.push(family);
        }
    }
    Ok(out)
}

#[derive(Default)]
struct VariantAliasIndex {
    forward: HashMap<WireString, WireString>,
    provider_scoped: HashMap<WireString, WireString>,
    reverse: HashMap<WireString, Arc<Mutex<Vec<WireString>>>>,
    family_ids: HashSet<WireString>,
}
impl VariantAliasIndex {
    fn add(&mut self, from: &WireString, to: &WireString) -> bool {
        let key = lower(from);
        if from == to || self.forward.contains_key(&key) {
            return false;
        }
        self.forward.insert(key, to.clone());
        self.reverse
            .entry(to.clone())
            .or_default()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(from.clone());
        true
    }
    fn add_family(&mut self, family: &EffortVariantFamily) -> Result<(), CollapseError> {
        let id = text(family, "id")?;
        self.family_ids.insert(id.clone());
        for source in strings(family.get("members")).into_iter().chain(strings(family.get("extraAliases"))) {
            self.add(&source, &id);
        }
        Ok(())
    }
    fn learn(&mut self, table: &VariantCollapseTable, id: &WireString) -> Result<(), CollapseError> {
        if let Some(templates) = &table.templates
            && let Some(family) = template_family_for(templates, id)?
            && !self.family_ids.contains(&text(&family, "id")?)
        {
            self.add_family(&family)?;
        }
        Ok(())
    }
    fn resolve(
        &mut self,
        table: Option<&VariantCollapseTable>,
        id: &WireString,
    ) -> Result<Option<WireString>, CollapseError> {
        let key = lower(id);
        if let Some(table) = table {
            self.learn(table, &key)?;
        }
        Ok(self.forward.get(&key).cloned())
    }
    fn is_family(&mut self, table: &VariantCollapseTable, id: &WireString) -> Result<bool, CollapseError> {
        self.learn(table, id)?;
        Ok(self.family_ids.contains(id))
    }
    fn sources(
        &mut self,
        table: Option<&VariantCollapseTable>,
        id: &WireString,
    ) -> Result<Option<Arc<Mutex<Vec<WireString>>>>, CollapseError> {
        if let Some(table) = table {
            self.learn(table, id)?;
        }
        Ok(self.reverse.get(id).cloned())
    }
}

/// Preserve one runtime for the Host's module lifetime. Recreating this value
/// is an explicit fresh-module boundary, not a catalog refresh operation.
pub struct CollapseRuntime {
    reviewed: Vec<(WireString, Arc<VariantCollapseTable>)>,
    static_indexes: HashMap<usize, VariantAliasIndex>,
    // Hold each indexed table alive so an allocator cannot recycle its identity.
    indexed_tables: Vec<Arc<VariantCollapseTable>>,
    dynamic: Vec<(WireString, VariantAliasIndex)>,
    policy: Arc<dyn CollapseModelPolicy>,
}
impl CollapseRuntime {
    pub fn new() -> Result<Self, CollapseError> {
        let vocabulary = catalog_rules::collapse_vocabulary();
        let mut tables: Vec<(WireString, VariantCollapseTable)> = Vec::new();
        for compiled in vocabulary["variantFamilies"].as_array().into_iter().flatten() {
            let provider = compiled["provider"]
                .as_str()
                .ok_or_else(|| CollapseError::new("compiled family provider must be string".into()))?;
            let key = WireString::from(provider);
            let index = tables.iter().position(|(existing, _)| existing == &key).unwrap_or_else(|| {
                tables.push((key, VariantCollapseTable::new(Vec::new())));
                tables.len() - 1
            });
            let table = &mut tables[index].1;
            if compiled["id"].as_str().is_some_and(|id| id.contains("{rev}")) {
                table.templates.get_or_insert_with(Vec::new).push(compiled_template(compiled)?);
            } else {
                table.families.push(compiled_family(compiled));
            }
        }
        if let Some(providers) = vocabulary["providerAliases"].as_object() {
            for (provider, aliases) in ara_prompt::js::entries(providers) {
                let key = WireString::from(provider.as_str());
                let index = tables.iter().position(|(existing, _)| existing == &key).unwrap_or_else(|| {
                    tables.push((key, VariantCollapseTable::new(Vec::new())));
                    tables.len() - 1
                });
                tables[index].1.provider_aliases = Some(VariantSpec::from_json(aliases).value);
            }
        }
        Ok(Self {
            reviewed: tables.into_iter().map(|(provider, table)| (provider, Arc::new(table))).collect(),
            static_indexes: HashMap::new(),
            indexed_tables: Vec::new(),
            dynamic: Vec::new(),
            policy: Arc::new(crate::model_wire_policy::WireModelPolicy),
        })
    }
    pub fn set_model_policy(&mut self, policy: Arc<dyn CollapseModelPolicy>) {
        self.policy = policy;
    }
    pub fn reviewed_providers(&self) -> Vec<WireString> {
        self.reviewed.iter().map(|(provider, _)| provider.clone()).collect()
    }
    pub fn reviewed_collapse_table(&self, provider: &WireString) -> ReviewedCollapseLookup {
        for key in [provider.clone(), lower(provider)] {
            if let Some((_, table)) = self.reviewed.iter().find(|(provider, _)| provider == &key) {
                return ReviewedCollapseLookup::Table(Arc::clone(table));
            }
            if let Some(property) = inherited_table_property(&key) {
                return ReviewedCollapseLookup::InheritedProperty(property);
            }
        }
        ReviewedCollapseLookup::None
    }
    fn declared_table(&self, provider: &WireString) -> Result<Option<Arc<VariantCollapseTable>>, CollapseError> {
        match self.reviewed_collapse_table(provider) {
            ReviewedCollapseLookup::Table(table) => Ok(Some(table)),
            ReviewedCollapseLookup::InheritedProperty(_) => {
                Err(CollapseError::new("undefined is not an object (evaluating 'family of table.families')".into()))
            }
            ReviewedCollapseLookup::None => Ok(None),
        }
    }
    fn alias_index(&mut self, table: &Arc<VariantCollapseTable>) -> Result<&mut VariantAliasIndex, CollapseError> {
        let identity = Arc::as_ptr(table) as usize;
        if !self.static_indexes.contains_key(&identity) {
            let mut index = VariantAliasIndex::default();
            for family in &table.families {
                index.add_family(family)?;
            }
            if let Some(aliases) = &table.provider_aliases {
                for (alias, target) in aliases.entries().unwrap_or_default() {
                    if let Some(target) = target.as_string()
                        && alias != target
                    {
                        index.provider_scoped.insert(lower(alias), target.clone());
                    }
                }
            }
            self.indexed_tables.push(Arc::clone(table));
            self.static_indexes.insert(identity, index);
        }
        Ok(self.static_indexes.get_mut(&identity).unwrap())
    }
}

fn field<'a>(spec: &'a VariantSpec, path: &[&str]) -> Option<&'a WireValue> {
    spec.get_path(&path.iter().map(|key| WireString::from(*key)).collect::<Vec<_>>())
}
fn route(family: &VariantSpec, key: &str) -> Option<WireString> {
    field(family, &["routing", key]).and_then(WireValue::as_string).cloned()
}
fn thinking_route(spec: &VariantSpec, key: &str) -> Option<WireString> {
    field(spec, &["thinking", "effortRouting", key]).and_then(WireValue::as_string).cloned()
}
fn request_id(spec: &VariantSpec) -> Option<WireString> {
    spec.get("requestModelId").and_then(WireValue::as_string).cloned()
}
fn retired_members(family: &VariantSpec) -> HashSet<WireString> {
    strings(family.get("retiredMembers")).into_iter().collect()
}
fn fresh_ref(spec: VariantSpec) -> SpecRef {
    Arc::new(spec)
}

fn reconcile_retired_routing(
    spec: &SpecRef,
    family: &EffortVariantFamily,
    retired: &HashSet<WireString>,
) -> Result<SpecRef, CollapseError> {
    let current = spec.record("thinking");
    let routing = current.as_ref().and_then(|thinking| thinking.record("effortRouting"));
    let request_retired = request_id(spec).is_some_and(|target| retired.contains(&target));
    let routing_retired = routing.as_ref().is_some_and(|routing| {
        ROUTE_KEYS
            .iter()
            .any(|key| routing.get(key).and_then(WireValue::as_string).is_some_and(|target| retired.contains(target)))
    });
    if !request_retired && !routing_retired {
        return Ok(Arc::clone(spec));
    }
    let fallback = route(family, "off")
        .filter(|target| !retired.contains(target))
        .or_else(|| strings(family.get("members")).into_iter().find(|target| !retired.contains(target)));
    let mut next = (**spec).clone();
    if routing_retired && let (Some(mut thinking), Some(routing)) = (current, routing) {
        let mut next_routing = object();
        for key in ROUTE_KEYS {
            let Some(target) = routing.get(key).and_then(WireValue::as_string) else { continue };
            let target = if retired.contains(target) {
                route(family, key).filter(|target| !retired.contains(target)).or_else(|| fallback.clone())
            } else {
                Some(target.clone())
            };
            if let Some(target) = target {
                next_routing.insert(key, value_string(target));
            }
        }
        thinking.set("effortRouting", next_routing);
        next.set_record("thinking", &thinking);
    }
    if request_retired {
        if let Some(target) = fallback.filter(|target| text(spec, "id").is_ok_and(|id| id != *target)) {
            next.set("requestModelId", value_string(target));
        } else {
            next.remove("requestModelId");
        }
    }
    Ok(fresh_ref(next))
}
fn refresh_collapsed_thinking(
    spec: &SpecRef,
    family: &EffortVariantFamily,
    retired: &HashSet<WireString>,
) -> Result<SpecRef, CollapseError> {
    let Some(mut thinking) = family.record("thinking") else { return Ok(Arc::clone(spec)) };
    if !spec.get("reasoning").is_some_and(truthy) || thinking.get("effortBudgets").is_none() {
        return Ok(Arc::clone(spec));
    }
    let mut routing = object();
    let mut has_routing = false;
    for key in ROUTE_KEYS {
        if let Some(target) = route(family, key).filter(|target| !retired.contains(target)) {
            routing.insert(key, value_string(target));
            has_routing = true;
        }
    }
    if has_routing {
        thinking.set("effortRouting", routing);
    }
    if family.get("suppressWhenOff").is_some_and(truthy) {
        thinking.set("suppressWhenOff", WireValue::Bool(true));
    }
    let request = route(family, "off")
        .filter(|target| !retired.contains(target) && text(spec, "id").is_ok_and(|id| id != *target))
        .or_else(|| request_id(spec));
    let current = spec.record("thinking");
    let equal = current.as_ref().is_some_and(|current| deep_equal_at(&thinking, current, &[]));
    if equal && request == request_id(spec) {
        return Ok(Arc::clone(spec));
    }
    let mut next = (**spec).clone();
    next.set_record("thinking", &thinking);
    if let Some(request) = request {
        next.set("requestModelId", value_string(request));
    }
    Ok(fresh_ref(next))
}
fn reconcile_default_member(
    spec: &SpecRef,
    family: &EffortVariantFamily,
    present: Option<&HashSet<WireString>>,
) -> Result<SpecRef, CollapseError> {
    let Some(default) = family.get("defaultMember").and_then(WireValue::as_string) else { return Ok(Arc::clone(spec)) };
    if default == &text(spec, "id")? {
        return Ok(Arc::clone(spec));
    }
    let target = if present.is_none_or(|present| present.contains(default)) {
        Some(default.clone())
    } else {
        strings(family.get("members"))
            .into_iter()
            .find(|member| present.is_some_and(|present| present.contains(member)))
    };
    let Some(target) = target else { return Ok(Arc::clone(spec)) };
    if request_id(spec).as_ref() == Some(&target) || field(spec, &["thinking", "effortRouting"]).is_none() {
        return Ok(Arc::clone(spec));
    }
    if ROUTE_KEYS.iter().any(|key| thinking_route(spec, key).as_ref() == Some(&target)) {
        let mut next = (**spec).clone();
        next.set("requestModelId", value_string(target));
        return Ok(fresh_ref(next));
    }
    Ok(Arc::clone(spec))
}
fn max_or_null(specs: &[SpecRef], key: &str) -> WireValue {
    let mut maximum: Option<f64> = None;
    for spec in specs {
        let Some(value) = spec.get(key).filter(|value| !matches!(value, WireValue::Null)) else { continue };
        let number = value.as_number().unwrap_or_else(|| {
            serde_json::from_str::<serde_json::Value>(&value.stringify())
                .map(|value| ara_prompt::js::to_number(&value))
                .unwrap_or(f64::NAN)
        });
        maximum = Some(match maximum {
            None => number,
            Some(previous) if previous.is_nan() || number.is_nan() => f64::NAN,
            Some(previous) if number > previous || (number == 0.0 && previous == 0.0 && !number.is_sign_negative()) => {
                number
            }
            Some(previous) => previous,
        });
    }
    maximum.map_or(WireValue::Null, WireValue::Number)
}
fn includes(value: Option<&WireValue>, needle: &str) -> bool {
    match value {
        Some(WireValue::Array(items)) => {
            items.iter().any(|item| item.as_string().is_some_and(|item| item.equals_ascii(needle)))
        }
        Some(WireValue::String(value)) => {
            let needle: Vec<_> = needle.encode_utf16().collect();
            value.units().windows(needle.len()).any(|window| window == needle)
        }
        _ => false,
    }
}
fn by_id(specs: &[SpecRef]) -> Result<(HashMap<WireString, SpecRef>, Vec<WireString>), CollapseError> {
    let mut by_id = HashMap::new();
    let mut ids = Vec::new();
    for spec in specs {
        let id = text(spec, "id")?;
        if let std::collections::hash_map::Entry::Vacant(entry) = by_id.entry(id) {
            ids.push(entry.key().clone());
            entry.insert(Arc::clone(spec));
        }
    }
    Ok((by_id, ids))
}
fn collapse_with_table(specs: &[SpecRef], table: &VariantCollapseTable) -> Result<Vec<SpecRef>, CollapseError> {
    let (by_id, ids) = by_id(specs)?;
    let instantiated = instantiate_templates(table, &ids)?;
    let mut replacement: HashMap<WireString, SpecRef> = HashMap::new();
    let mut family_by_spec: HashMap<WireString, WireString> = HashMap::new();
    for family in table.families.iter().chain(instantiated.iter()) {
        let family_id = text(family, "id")?;
        let retired = retired_members(family);
        let existing = by_id.get(&family_id);
        let existing_collapsed = existing.is_some_and(|existing| {
            existing.get("requestModelId").is_some() || field(existing, &["thinking", "effortRouting"]).is_some()
        });
        let reconciled = match existing {
            Some(existing) if existing_collapsed && !retired.is_empty() => {
                Some(reconcile_retired_routing(existing, family, &retired)?)
            }
            Some(existing) => Some(Arc::clone(existing)),
            None => None,
        };
        let raw_present: Vec<_> = strings(family.get("members"))
            .into_iter()
            .filter(|id| by_id.contains_key(id) && !(id == &family_id && existing_collapsed))
            .collect();
        if raw_present.is_empty() {
            let refreshed = if existing_collapsed {
                match &reconciled {
                    Some(reconciled) => Some(reconcile_default_member(
                        &refresh_collapsed_thinking(reconciled, family, &retired)?,
                        family,
                        None,
                    )?),
                    None => None,
                }
            } else {
                reconciled
            };
            if let (Some(refreshed), Some(existing)) = (refreshed, existing)
                && !Arc::ptr_eq(&refreshed, existing)
            {
                family_by_spec.insert(family_id.clone(), family_id.clone());
                replacement.insert(family_id, refreshed);
            }
            continue;
        }
        for id in &raw_present {
            family_by_spec.insert(id.clone(), family_id.clone());
        }
        if existing.is_some() {
            family_by_spec.insert(family_id.clone(), family_id.clone());
        }
        let present: HashSet<_> = raw_present.iter().cloned().collect();
        if existing_collapsed {
            if let Some(reconciled) = reconciled {
                replacement.insert(family_id, reconcile_default_member(&reconciled, family, Some(&present))?);
            }
            continue;
        }
        let member_specs: Vec<_> = raw_present.iter().filter_map(|id| by_id.get(id).cloned()).collect();
        let Some(first) = member_specs.first() else { continue };
        let mut routing = object();
        let mut has_routing = false;
        let mut has_effort = false;
        let mut used_absent = false;
        for key in ROUTE_KEYS {
            let Some(target) = route(family, key) else { continue };
            let target_present = present.contains(&target);
            let preserve_absent =
                *key != "off" && matches!(family.get("preserveAbsentEffortRoutes"), Some(WireValue::Bool(true)));
            if (target_present || preserve_absent) && !retired.contains(&target) {
                routing.insert(key, value_string(target));
                has_routing = true;
                if *key != "off" {
                    has_effort = true;
                    if !target_present {
                        used_absent = true;
                    }
                }
            }
        }
        let reasoning = member_specs.iter().any(|spec| spec.get("reasoning").is_some_and(truthy)) || has_effort;
        let mut thinking = family.record("thinking").filter(|thinking| truthy(&thinking.value));
        if let Some(thinking) = &mut thinking {
            if has_routing {
                thinking.set("effortRouting", routing);
            }
            if family.get("suppressWhenOff").is_some_and(truthy) {
                thinking.set("suppressWhenOff", WireValue::Bool(true));
            }
        }
        let mut input = Vec::new();
        for modality in ["text", "image"] {
            if member_specs.iter().any(|spec| includes(spec.get("input"), modality)) {
                input.push(WireString::from(modality));
            }
        }
        let mut collapsed = (**first).clone();
        collapsed.set("id", value_string(family_id.clone()));
        collapsed.set("name", value_string(text(family, "name")?));
        collapsed.set("reasoning", WireValue::Bool(reasoning));
        collapsed.set("input", string_array(&input));
        collapsed.set("contextWindow", max_or_null(&member_specs, "contextWindow"));
        collapsed.set("maxTokens", max_or_null(&member_specs, "maxTokens"));
        let preferred = family
            .get("defaultMember")
            .and_then(WireValue::as_string)
            .filter(|member| present.contains(*member) && !retired.contains(*member))
            .cloned();
        let Some(default) = preferred
            .or_else(|| raw_present.iter().find(|member| !retired.contains(*member)).cloned())
            .or_else(|| raw_present.first().cloned())
        else {
            continue;
        };
        if default == family_id && !used_absent {
            collapsed.remove("requestModelId");
        } else {
            collapsed.set("requestModelId", value_string(default));
        }
        if reasoning {
            if let Some(thinking) = thinking {
                collapsed.set_record("thinking", &thinking);
            } else {
                collapsed.remove("thinking");
            }
        } else {
            collapsed.remove("thinking");
        }
        replacement.insert(family_id, fresh_ref(collapsed));
    }
    // Recycled aliases retain their own live identity, including when the
    // canonical family row exists alongside them.
    for family in &table.families {
        let family_id = text(family, "id")?;
        let retired = retired_members(family);
        for alias in strings(family.get("extraAliases")) {
            if alias == family_id || family_by_spec.contains_key(&alias) {
                continue;
            }
            if let Some(existing) = by_id.get(&alias) {
                let refreshed = refresh_collapsed_thinking(existing, family, &retired)?;
                if !Arc::ptr_eq(&refreshed, existing) {
                    family_by_spec.insert(alias.clone(), alias.clone());
                    replacement.insert(alias, refreshed);
                }
            }
        }
    }
    if replacement.is_empty() {
        return Ok(specs.to_vec());
    }
    let mut emitted = HashSet::new();
    let mut out = Vec::new();
    for spec in specs {
        let id = text(spec, "id")?;
        match family_by_spec.get(&id) {
            None => out.push(Arc::clone(spec)),
            Some(family) if emitted.insert(family.clone()) => {
                if let Some(replacement) = replacement.get(family) {
                    out.push(Arc::clone(replacement));
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

struct CursorMember {
    spec: SpecRef,
    tier: &'static str,
}
struct CursorGroup {
    base: WireString,
    fast: bool,
    members: Vec<CursorMember>,
}
fn collapsed_cursor_logical_matches(spec: &VariantSpec, members: &[CursorMember]) -> bool {
    if !field(spec, &["thinking", "effortRouting"]).is_some_and(truthy) {
        return false;
    }
    members.iter().all(|member| {
        let id = member.spec.get("id");
        strict_equal(spec.get("requestModelId"), id)
            || ROUTE_KEYS.iter().any(|key| strict_equal(field(spec, &["thinking", "effortRouting", key]), id))
    })
}
fn tier_family(id: WireString, name: WireString, routes: &WireValue, efforts: &[&str]) -> EffortVariantFamily {
    let mut routing = object();
    if let Some(off) = routes.get("off").filter(|value| truthy(value)) {
        routing.insert("off", off.clone());
    }
    for effort in efforts {
        if let Some(target) = routes.get(effort).filter(|value| truthy(value)) {
            routing.insert(effort, target.clone());
        }
    }
    let mut members = Vec::new();
    for key in ROUTE_KEYS {
        if let Some(target) = routes.get(key).and_then(WireValue::as_string)
            && !members.contains(target)
        {
            members.push(target.clone());
        }
    }
    let mut thinking = WireValue::object(vec![("mode", ascii_string("effort")), ("efforts", ascii_array(efforts))]);
    if !routes.get("off").is_some_and(truthy) {
        thinking.insert("requiresEffort", WireValue::Bool(true));
    }
    VariantSpec::from_wire(WireValue::object(vec![
        ("id", value_string(id)),
        ("name", value_string(name)),
        ("members", string_array(&members)),
        ("routing", routing),
        ("thinking", thinking),
    ]))
}
fn derive_cursor_effort_families(specs: &[SpecRef]) -> Result<Vec<EffortVariantFamily>, CollapseError> {
    let (by_id, _) = by_id(specs)?;
    let mut id_pattern = make_regex("^(.+?)-(extra-high|none|minimal|low|medium|high|xhigh|max)(-fast)?$".into(), "")?;
    let mut base_pattern = make_regex("-(extra-high|none|minimal|low|medium|high|xhigh|max)$".into(), "")?;
    let mut thinking_pattern = make_regex("(^|-)thinking($|-)".into(), "")?;
    let mut name_pattern =
        make_regex("\\s+(extra-high|none|minimal|low|medium|high|xhigh|max)(\\s+fast)?$".into(), "i")?;
    let mut groups: Vec<CursorGroup> = Vec::new();
    let mut candidates = HashSet::new();
    for spec in specs {
        let Some(hit) = id_pattern.exec(&text(spec, "id")?) else { continue };
        let Some(base) = hit.captures.get(1).and_then(Option::as_ref).filter(|value| !value.is_empty()).cloned() else {
            continue;
        };
        let Some(token) = hit.captures.get(2).and_then(Option::as_ref) else { continue };
        let tier = if token.equals_ascii("extra-high") {
            "xhigh"
        } else {
            let Some(tier) = ["none", "minimal", "low", "medium", "high", "xhigh", "max"]
                .into_iter()
                .find(|tier| token.equals_ascii(tier))
            else {
                continue;
            };
            tier
        };
        let fast = hit.captures.get(3).is_some_and(Option::is_some);
        let index = groups.iter().position(|group| group.base == base && group.fast == fast).unwrap_or_else(|| {
            groups.push(CursorGroup { base: base.clone(), fast, members: Vec::new() });
            groups.len() - 1
        });
        groups[index].members.push(CursorMember { spec: Arc::clone(spec), tier });
        candidates.insert(base);
    }
    let mut unsafe_bases = HashSet::new();
    for group in &groups {
        let standard = groups
            .iter()
            .find(|candidate| candidate.base == group.base && !candidate.fast)
            .map_or(&[][..], |group| group.members.as_slice());
        let independent_standard =
            by_id.get(&group.base).is_some_and(|base| !collapsed_cursor_logical_matches(base, standard));
        let lane = append(&group.base, if group.fast { "-fast" } else { "" });
        let independent_lane =
            by_id.get(&lane).is_some_and(|base| !collapsed_cursor_logical_matches(base, &group.members));
        let distinct: HashSet<_> = group.members.iter().map(|member| member.tier).collect();
        if independent_standard
            || independent_lane
            || distinct.len() != group.members.len()
            || base_pattern.exec(&group.base).is_some()
            || thinking_pattern.exec(&group.base).is_some()
            || candidates.contains(&append(&group.base, "-thinking"))
            || ["-thinking", "-thinking-fast", "-fast-thinking"]
                .iter()
                .any(|suffix| by_id.contains_key(&append(&group.base, suffix)))
            || group
                .members
                .iter()
                .any(|member| member.spec.get("thinking").is_some() || member.spec.get("requestModelId").is_some())
            || group
                .members
                .iter()
                .any(|member| candidates.contains(&append(&group.base, &format!("-{}", member.tier))))
        {
            unsafe_bases.insert(group.base.clone());
        }
    }
    let mut families = Vec::new();
    for group in &groups {
        if group.members.len() < 2 || unsafe_bases.contains(&group.base) {
            continue;
        }
        let first = &group.members[0].spec;
        let incompatible = group.members.iter().any(|member| {
            ["api", "baseUrl", "contextWindow", "maxTokens", "cursorMaxMode"]
                .iter()
                .any(|key| !strict_equal(member.spec.get(key), first.get(key)))
                || ["cost", "compat"].iter().any(|key| !deep_equal_at(&member.spec, first, &[WireString::from(*key)]))
        });
        if incompatible {
            continue;
        }
        let mut routes = object();
        for member in &group.members {
            routes.insert(
                if member.tier == "none" { "off" } else { member.tier },
                value_string(text(&member.spec, "id")?),
            );
        }
        let efforts: Vec<_> = EFFORTS.iter().copied().filter(|effort| routes.get(effort).is_some()).collect();
        if efforts.is_empty() {
            continue;
        }
        let name = text(first, "name")?;
        let stripped = trim(&name_pattern.exec(&name).map_or_else(|| name.clone(), |hit| name.slice_prefix(hit.index)));
        let base_name =
            if stripped == text(first, "id")? || stripped.is_empty() { group.base.clone() } else { stripped };
        let id = append(&group.base, if group.fast { "-fast" } else { "" });
        let name = append(&base_name, if group.fast { " Fast" } else { "" });
        families.push(tier_family(id, name, &routes, &efforts));
    }
    Ok(families)
}

pub(crate) fn strip_thinking_variant(model: &WireString) -> Option<WireString> {
    let lowered = lower(model);
    for token in catalog_rules::collapse_vocabulary()["pairTokens"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
    {
        let needle: Vec<_> = format!("-{token}").encode_utf16().collect();
        let mut search = 0;
        while search < lowered.len() {
            let Some(offset) = lowered.units()[search..].windows(needle.len()).position(|window| window == needle)
            else {
                break;
            };
            let index = search + offset;
            let end = index + needle.len();
            let token_character =
                |unit: u16| (b'0' as u16..=b'9' as u16).contains(&unit) || (b'a' as u16..=b'z' as u16).contains(&unit);
            let followed = lowered.units().get(end).copied().is_some_and(token_character);
            let mut start = index;
            while start > 0 && token_character(lowered.units()[start - 1]) {
                start -= 1;
            }
            let preceding = WireString::from_units(lowered.units()[start..index].to_vec());
            if !followed && !preceding.equals_ascii("non") && !preceding.equals_ascii("no") {
                let mut units = model.units()[..index.min(model.len())].to_vec();
                units.extend(&model.units()[end.min(model.len())..]);
                if !units.is_empty() {
                    return Some(WireString::from_units(units));
                }
                return None;
            }
            search = index + 1;
        }
    }
    None
}
fn provider_thinking_base(provider: &WireString, model: &WireString) -> Option<WireString> {
    let vocabulary = catalog_rules::collapse_vocabulary();
    let lowered = lower(model);
    let provider = lower(provider);
    // The first reviewed effort family short-circuits suffix classification.
    for family in vocabulary["effortFamilies"].as_array().into_iter().flatten() {
        let matches = family["provider"].as_str().is_some_and(|value| provider.equals_ascii(value))
            && family["aliases"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .any(|alias| lowered.equals_ascii(alias));
        if matches {
            return None;
        }
    }
    let bare_start = lowered.units().iter().rposition(|unit| *unit == b'/' as u16).map_or(0, |index| index + 1);
    let bare = &lowered.units()[bare_start..];
    let mut winner: Option<&serde_json::Value> = None;
    for rule in vocabulary["suffixes"].as_array().into_iter().flatten() {
        let suffix: Vec<_> = rule["suffix"].as_str().unwrap_or_default().encode_utf16().collect();
        if !lowered.units().ends_with(&suffix) {
            continue;
        }
        if rule["exceptBarePrefix"]
            .as_str()
            .is_some_and(|prefix| bare.starts_with(&prefix.encode_utf16().collect::<Vec<_>>()))
        {
            continue;
        }
        let previous_len =
            winner.and_then(|winner| winner["suffix"].as_str()).map_or(0, |suffix| suffix.encode_utf16().count());
        if winner.is_none() || suffix.len() > previous_len {
            winner = Some(rule);
        }
    }
    let winner = winner?;
    if winner["thinking"] != true {
        return None;
    }
    let length = winner["suffix"].as_str().unwrap_or_default().encode_utf16().count();
    Some(model.slice_prefix(model.len().saturating_sub(length)))
}
fn thinking_surface(mut thinking: VariantSpec) -> VariantSpec {
    for key in ["effortRouting", "suppressWhenOff", "requiresEffort"] {
        thinking.remove(key);
    }
    thinking
}
impl CollapseRuntime {
    fn derive_pair_surface(&self, thinking: &VariantSpec, bare: &VariantSpec) -> Result<VariantSpec, CollapseError> {
        let baked =
            if is_nullish(thinking.get("thinking")) { bare.record("thinking") } else { thinking.record("thinking") };
        if let Some(baked) = baked.filter(|baked| {
            truthy(&baked.value)
                && baked.get("efforts").and_then(WireValue::as_array).is_some_and(|efforts| !efforts.is_empty())
        }) {
            return Ok(thinking_surface(baked));
        }
        let mut policy_spec = thinking.clone();
        policy_spec.remove("compat");
        policy_spec.set("reasoning", WireValue::Bool(true));
        policy_spec.set_undefined("thinking");
        let derived = self.policy.resolve(&policy_spec)?.record("thinking");
        if let Some(derived) = derived.filter(|derived| {
            truthy(&derived.value)
                && derived.get("efforts").and_then(WireValue::as_array).is_some_and(|efforts| !efforts.is_empty())
        }) {
            return Ok(thinking_surface(derived));
        }
        Ok(VariantSpec::from_wire(WireValue::object(vec![
            ("mode", ascii_string("budget")),
            ("efforts", ascii_array(DEFAULT_PAIR_EFFORTS)),
        ])))
    }
    pub fn derive_thinking_pair_families(
        &mut self,
        specs: &[SpecRef],
        table: Option<&Arc<VariantCollapseTable>>,
        provider: Option<&WireString>,
    ) -> Result<Vec<EffortVariantFamily>, CollapseError> {
        let (by_id, _) = by_id(specs)?;
        let mut families = Vec::new();
        for spec in specs {
            let id = text(spec, "id")?;
            let base_id = provider
                .and_then(|provider| provider_thinking_base(provider, &id))
                .or_else(|| strip_thinking_variant(&id));
            let Some(base_id) = base_id.filter(|base_id| base_id != &id) else { continue };
            let Some(base) = by_id.get(&base_id) else { continue };
            if let Some(table) = table {
                let index = self.alias_index(table)?;
                if index.resolve(Some(table), &id)?.is_some()
                    || index.resolve(Some(table), &base_id)?.is_some()
                    || index.is_family(table, &id)?
                    || index.is_family(table, &base_id)?
                {
                    continue;
                }
            }
            if !strict_equal(spec.get("api"), base.get("api")) {
                continue;
            }
            let zero = WireValue::Number(0.0);
            let priced = |spec: &VariantSpec| {
                !strict_equal(field(spec, &["cost", "input"]), Some(&zero))
                    || !strict_equal(field(spec, &["cost", "output"]), Some(&zero))
            };
            if priced(spec)
                && priced(base)
                && ["input", "output", "cacheRead", "cacheWrite"]
                    .iter()
                    .any(|key| !strict_equal(field(spec, &["cost", key]), field(base, &["cost", key])))
            {
                continue;
            }
            let surface = self.derive_pair_surface(spec, base)?;
            let mut routing = object();
            routing.insert("off", value_string(text(base, "id")?));
            for effort in strings(surface.get("efforts")) {
                let key = effort.to_utf8().map_err(|error| CollapseError::new(error.to_string()))?;
                routing.insert(&key, value_string(id.clone()));
            }
            let mut family = VariantSpec::from_wire(WireValue::object(vec![
                ("id", value_string(text(base, "id")?)),
                ("name", value_string(text(base, "name")?)),
                ("members", string_array(&[text(base, "id")?, id])),
                ("routing", routing),
            ]));
            family.set_record("thinking", &surface);
            families.push(family);
        }
        Ok(families)
    }
    pub fn is_collapsed_variant_spec(&mut self, spec: &VariantSpec) -> Result<bool, CollapseError> {
        if field(spec, &["thinking", "effortRouting"]).is_some() {
            return Ok(true);
        }
        if spec.get("requestModelId").is_none() {
            return Ok(false);
        }
        let Some(table) = self.declared_table(&text(spec, "provider")?)? else { return Ok(false) };
        self.alias_index(&table)?.is_family(&table, &text(spec, "id")?)
    }
}

impl CollapseRuntime {
    fn resolve_registered_alias(
        &mut self,
        provider: &WireString,
        model: &WireString,
    ) -> Result<Option<WireString>, CollapseError> {
        let provider_id = lower(provider);
        if let Some(table) = self.declared_table(provider)?
            && let Some(target) = self.alias_index(&table)?.resolve(Some(&table), model)?
        {
            return Ok(Some(target));
        }
        match self.dynamic.iter_mut().find(|(key, _)| key == &provider_id) {
            Some((_, index)) => index.resolve(None, model),
            None => Ok(None),
        }
    }
    fn register_collapsed_aliases(&mut self, provider: &WireString, specs: &[SpecRef]) -> Result<(), CollapseError> {
        let key = lower(provider);
        let existing = self.dynamic.iter().position(|(provider, _)| provider == &key);
        // Work against the original index without creating an empty provider
        // slot: its first registration determines bare-provider iteration order.
        let mut pending = None;
        for spec in specs {
            if !field(spec, &["thinking", "effortRouting"]).is_some_and(truthy) {
                continue;
            }
            let id = text(spec, "id")?;
            let mut registered = false;
            for effort in ROUTE_KEYS {
                let Some(source) = thinking_route(spec, effort).filter(|source| !source.is_empty() && source != &id)
                else {
                    continue;
                };
                let index = match existing {
                    Some(position) => &mut self.dynamic[position].1,
                    None => pending.get_or_insert_with(VariantAliasIndex::default),
                };
                registered = index.add(&source, &id) || registered;
            }
            if let Some(source) = request_id(spec).filter(|source| !source.is_empty() && source != &id) {
                let index = match existing {
                    Some(position) => &mut self.dynamic[position].1,
                    None => pending.get_or_insert_with(VariantAliasIndex::default),
                };
                registered = index.add(&source, &id) || registered;
            }
            if registered {
                let index = match existing {
                    Some(position) => &mut self.dynamic[position].1,
                    None => pending.as_mut().unwrap(),
                };
                index.family_ids.insert(id);
            }
        }
        if let Some(index) = pending {
            self.dynamic.push((key, index));
        }
        Ok(())
    }
    pub fn resolve_variant_selector(
        &mut self,
        provider: &WireString,
        model_id: &WireString,
    ) -> Result<Option<WireString>, CollapseError> {
        let normalized = lower(&trim(model_id));
        if let Some(registered) = self.resolve_registered_alias(provider, &normalized)? {
            return Ok(Some(registered));
        }
        let Some(table) = self.declared_table(provider)? else { return Ok(None) };
        Ok(self.alias_index(&table)?.provider_scoped.get(&normalized).cloned())
    }
    pub fn resolve_bare_variant_selector(
        &mut self,
        model_id: &WireString,
    ) -> Result<Option<BareVariantAliasHit>, CollapseError> {
        let normalized = lower(&trim(model_id));
        let mut providers = self.reviewed_providers();
        for (provider, _) in &self.dynamic {
            if !providers.contains(provider) {
                providers.push(provider.clone());
            }
        }
        for provider in &providers {
            let Some(hit) = self.resolve_registered_alias(provider, &normalized)? else { continue };
            let mut declaring = Vec::new();
            for candidate in &providers {
                if self.resolve_registered_alias(candidate, &normalized)?.as_ref() == Some(&hit) {
                    declaring.push(candidate.clone());
                }
            }
            return Ok(Some(BareVariantAliasHit { id: hit, providers: declaring }));
        }
        Ok(None)
    }
    pub fn get_variant_alias_sources(
        &mut self,
        provider: &WireString,
        model_id: &WireString,
    ) -> Result<VariantAliasSources, CollapseError> {
        let provider_id = lower(provider);
        let static_sources = match self.declared_table(provider)? {
            Some(table) => self.alias_index(&table)?.sources(Some(&table), model_id)?,
            None => None,
        };
        let dynamic_sources = match self.dynamic.iter_mut().find(|(key, _)| key == &provider_id) {
            Some((_, index)) => index.sources(None, model_id)?,
            None => None,
        };
        match (static_sources, dynamic_sources) {
            (Some(static_sources), Some(dynamic_sources)) => {
                let mut combined = Vec::new();
                for source in VariantAliasSources::Shared(static_sources)
                    .snapshot()
                    .into_iter()
                    .chain(VariantAliasSources::Shared(dynamic_sources).snapshot())
                {
                    if !combined.contains(&source) {
                        combined.push(source);
                    }
                }
                Ok(VariantAliasSources::Snapshot(Arc::new(Mutex::new(combined))))
            }
            (Some(sources), None) | (None, Some(sources)) => Ok(VariantAliasSources::Shared(sources)),
            (None, None) => Ok(VariantAliasSources::Snapshot(Arc::new(Mutex::new(Vec::new())))),
        }
    }
    fn resolve_collapsed_reference(
        &mut self,
        target: Option<&WireValue>,
        current_provider: &WireString,
        live: &HashMap<WireString, HashSet<WireString>>,
    ) -> Result<Option<WireString>, CollapseError> {
        let Some(target) = target else { return Ok(None) };
        let target =
            target.as_string().ok_or_else(|| CollapseError::new("model reference target must be a string".into()))?;
        let separator = target.units().iter().position(|unit| *unit == b'/' as u16);
        let provider = separator.map_or_else(|| current_provider.clone(), |index| target.slice_prefix(index));
        let model = separator
            .map_or_else(|| target.clone(), |index| WireString::from_units(target.units()[index + 1..].to_vec()));
        let normalized = lower(&trim(&model));
        let live_ids = live.get(&lower(&provider));
        if live_ids.is_some_and(|ids| ids.contains(&normalized)) {
            return Ok(Some(target.clone()));
        }
        let Some(alias) = self.resolve_registered_alias(&provider, &normalized)? else {
            return Ok(Some(target.clone()));
        };
        if !live_ids.is_some_and(|ids| ids.contains(&lower(&alias))) {
            return Ok(Some(target.clone()));
        }
        Ok(Some(if separator.is_some() { concat(&append(&provider, "/"), &alias) } else { alias }))
    }
    fn retarget_collapsed_references(&mut self, specs: &mut [SpecRef]) -> Result<(), CollapseError> {
        let mut live: HashMap<WireString, HashSet<WireString>> = HashMap::new();
        for spec in specs.iter() {
            live.entry(lower(&text(spec, "provider")?)).or_default().insert(lower(&text(spec, "id")?));
        }
        for spec in specs.iter_mut() {
            let provider = text(spec, "provider")?;
            let promotion = self.resolve_collapsed_reference(spec.get("contextPromotionTarget"), &provider, &live)?;
            let compaction = self.resolve_collapsed_reference(spec.get("compactionModel"), &provider, &live)?;
            let old_promotion = spec.get("contextPromotionTarget").and_then(WireValue::as_string);
            let old_compaction = spec.get("compactionModel").and_then(WireValue::as_string);
            if promotion.as_ref() == old_promotion && compaction.as_ref() == old_compaction {
                continue;
            }
            let mut next = (**spec).clone();
            for (key, target) in [("contextPromotionTarget", promotion), ("compactionModel", compaction)] {
                match target {
                    Some(target) => next.set(key, value_string(target)),
                    None => next.set_undefined(key),
                }
            }
            *spec = fresh_ref(next);
        }
        Ok(())
    }
    pub fn collapse_variants(
        &mut self,
        specs: &[SpecRef],
        table: Option<&Arc<VariantCollapseTable>>,
    ) -> Result<Vec<SpecRef>, CollapseError> {
        if let Some(table) = table {
            return collapse_with_table(specs, table);
        }
        let mut groups: Vec<(WireString, Vec<SpecRef>)> = Vec::new();
        for spec in specs {
            let provider = text(spec, "provider")?;
            let index = groups.iter().position(|(existing, _)| existing == &provider).unwrap_or_else(|| {
                groups.push((provider, Vec::new()));
                groups.len() - 1
            });
            groups[index].1.push(Arc::clone(spec));
        }
        let mut out = Vec::new();
        for (provider, slice) in groups {
            let table = self.declared_table(&provider).map_err(|error| {
                if error.name() == "TypeError" {
                    CollapseError::new("undefined is not an object (evaluating 'family of families')".into())
                } else {
                    error
                }
            })?;
            let mut result = match &table {
                Some(table) => collapse_with_table(&slice, table)?,
                None => slice,
            };
            if provider.equals_ascii("cursor") {
                let derived = derive_cursor_effort_families(&result)?;
                if !derived.is_empty() {
                    result = collapse_with_table(&result, &VariantCollapseTable::new(derived))?;
                }
            }
            let derived = self.derive_thinking_pair_families(&result, table.as_ref(), Some(&provider))?;
            if !derived.is_empty() {
                result = collapse_with_table(&result, &VariantCollapseTable::new(derived))?;
            }
            self.register_collapsed_aliases(&provider, &result)?;
            out.extend(result);
        }
        self.retarget_collapsed_references(&mut out)?;
        Ok(out)
    }
    pub fn collapse_built_variants(&mut self, models: &[SpecRef]) -> Result<Vec<SpecRef>, CollapseError> {
        let collapsed = self.collapse_variants(models, None)?;
        let input_refs: HashSet<_> = models.iter().map(|model| Arc::as_ptr(model) as usize).collect();
        collapsed
            .into_iter()
            .map(|model| {
                if input_refs.contains(&(Arc::as_ptr(&model) as usize)) {
                    return Ok(model);
                }
                self.policy.build(&project_model_spec(&model)).map(fresh_ref)
            })
            .collect()
    }
}
fn project_model_spec(model: &VariantSpec) -> VariantSpec {
    let compat = model.record("compatConfig");
    let mut spec = model.clone();
    for key in ["compat", "compatConfig", "identity", "requiresGlyphTokenization", "supportsComputerUseConfig"] {
        spec.remove(key);
    }
    match compat {
        Some(compat) => spec.set_record("compat", &compat),
        None => spec.set_undefined("compat"),
    }
    spec
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn model(provider: &str, id: &str) -> SpecRef {
        fresh_ref(VariantSpec::from_json(&json!({
            "provider":provider,"id":id,"name":id,"api":"openai-completions","baseUrl":"https://example.test/v1",
            "reasoning":false,"input":["text"],"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0},
            "contextWindow":100,"maxTokens":50
        })))
    }
    fn routed_model(provider: &str, id: &str, source: &str) -> SpecRef {
        let mut spec = (*model(provider, id)).clone();
        spec.set(
            "thinking",
            WireValue::object(vec![
                ("mode", ascii_string("budget")),
                ("efforts", ascii_array(DEFAULT_PAIR_EFFORTS)),
                ("effortRouting", WireValue::object(vec![("high", ascii_string(source))])),
            ]),
        );
        fresh_ref(spec)
    }

    #[test]
    fn explicit_table_preserves_priority_metadata_identity_and_does_not_register() {
        let mut runtime = CollapseRuntime::new().unwrap();
        let bare = model("test-provider", "m");
        let mut expensive = (*model("test-provider", "m-thinking")).clone();
        expensive.set("cost", WireValue::object(vec![("input", WireValue::Number(90.0))]));
        let thinking = fresh_ref(expensive);
        let untouched = model("test-provider", "observer");
        let mut observer = (*untouched).clone();
        observer.set("contextPromotionTarget", ascii_string("m-thinking"));
        let observer = fresh_ref(observer);
        let family = VariantSpec::from_json(&json!({"id":"logical","name":"Logical","members":["m","m-thinking"],
            "routing":{"off":"m","high":"m-thinking"},"thinking":{"mode":"budget","efforts":["high"]}}));
        let table = Arc::new(VariantCollapseTable::new(vec![family]));
        let collapsed =
            runtime.collapse_variants(&[thinking, Arc::clone(&observer), Arc::clone(&bare)], Some(&table)).unwrap();
        assert_eq!(text(&collapsed[0], "id").unwrap(), WireString::from("logical"));
        assert!(
            field(&collapsed[0], &["cost", "input"]).unwrap().deep_equal(field(&bare, &["cost", "input"]).unwrap())
        );
        assert!(Arc::ptr_eq(&collapsed[1], &observer));
        assert!(runtime.resolve_variant_selector(&"test-provider".into(), &"m-thinking".into()).unwrap().is_none());
        assert_eq!(text(&collapsed[1], "contextPromotionTarget").unwrap(), WireString::from("m-thinking"));
    }

    #[test]
    fn reverse_single_index_view_stays_live_and_first_forward_claim_wins() {
        let mut runtime = CollapseRuntime::new().unwrap();
        let provider = WireString::from("dynamic-test");
        let target = WireString::from("logical");
        runtime.collapse_variants(&[routed_model("dynamic-test", "logical", "wire-one")], None).unwrap();
        let view = runtime.get_variant_alias_sources(&provider, &target).unwrap();
        let second_view = runtime.get_variant_alias_sources(&provider, &target).unwrap();
        assert!(view.shares_storage(&second_view));
        runtime.collapse_variants(&[routed_model("dynamic-test", "logical", "wire-two")], None).unwrap();
        assert_eq!(view.snapshot(), vec![WireString::from("wire-one"), WireString::from("wire-two")]);
        runtime.collapse_variants(&[routed_model("dynamic-test", "other-logical", "wire-one")], None).unwrap();
        assert_eq!(runtime.resolve_variant_selector(&provider, &" WIRE-ONE ".into()).unwrap(), Some(target));
        let request_only = {
            let mut spec = (*model("dynamic-test", "request-only")).clone();
            spec.set("requestModelId", ascii_string("request-wire"));
            fresh_ref(spec)
        };
        runtime.collapse_variants(&[request_only], None).unwrap();
        assert!(runtime.resolve_variant_selector(&provider, &"request-wire".into()).unwrap().is_none());
    }

    #[test]
    fn null_and_undefined_routing_are_distinct_and_lossless_pair_keys_survive() {
        let mut runtime = CollapseRuntime::new().unwrap();
        let mut spec = (*model("undefined-test", "plain")).clone();
        spec.set("thinking", WireValue::object(vec![("effortRouting", WireValue::Null)]));
        assert!(runtime.is_collapsed_variant_spec(&spec).unwrap());
        spec.undefined_paths.push(vec!["thinking".into(), "effortRouting".into()]);
        assert!(!runtime.is_collapsed_variant_spec(&spec).unwrap());
        let expected = WireString::from_units(vec![0x0130, b'-' as u16, 0xde00]);
        assert_eq!(strip_thinking_variant(&WireString::from("İ-thinking😀")), Some(expected.clone()));
        let mut bare = (*model("wire-test", "temporary-bare")).clone();
        bare.set("id", value_string(expected.clone()));
        let mut thinking = (*model("wire-test", "İ-thinking😀")).clone();
        thinking.set(
            "thinking",
            WireValue::object(vec![("mode", ascii_string("budget")), ("efforts", ascii_array(DEFAULT_PAIR_EFFORTS))]),
        );
        let collapsed = runtime.collapse_variants(&[fresh_ref(bare), fresh_ref(thinking)], None).unwrap();
        assert_eq!(collapsed.len(), 1);
        assert_eq!(text(&collapsed[0], "id").unwrap(), expected);
        assert!(text(&collapsed[0], "id").unwrap().to_utf8().is_err());
    }

    #[test]
    fn untouched_built_rows_keep_reference_and_opaque_metadata_never_becomes_null() {
        let mut runtime = CollapseRuntime::new().unwrap();
        let mut spec = (*model("opaque-test", "plain")).clone();
        spec.set("opaque", WireValue::String(WireString::from_units(vec![0xd800])));
        spec.set("opaqueNumber", WireValue::Number(f64::NAN));
        spec.set_undefined("opaqueUndefined");
        let spec = fresh_ref(spec);
        let output = runtime.collapse_built_variants(&[Arc::clone(&spec)]).unwrap();
        assert!(Arc::ptr_eq(&spec, &output[0]));
        let projected = project_model_spec(&spec);
        let built = JsonModelPolicy.build(&projected).unwrap();
        assert!(built.get("opaqueNumber").unwrap().as_number().unwrap().is_nan());
        assert_eq!(built.get("opaque").unwrap().as_string().unwrap().units(), &[0xd800]);
        assert!(built.own_keys().contains(&WireString::from("opaqueUndefined")));
        assert!(built.get("opaqueUndefined").is_none());
    }
}
