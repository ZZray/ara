//! Canonical computer metadata, from fixed OMP agent-loop.ts:209-352 and
//! ai/types.ts:804-869 at 596f2da7101178214aa27a753529d15e6b7ad91d.
//! Concrete provider encoders remain a separate surface.
//!
//! MIT License
//! Copyright (c) 2025 Mario Zechner
//! Copyright (c) 2025-2026 Can Bölük
//! Copyright (c) 2026 Stencil Labs, Inc.
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in all
//! copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//! SOFTWARE.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolCallProviderMetadata {
    #[serde(rename = "computer")]
    Computer(ComputerToolCallMetadata),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolResultProviderMetadata {
    #[serde(rename = "computer")]
    Computer(ComputerToolResultMetadata),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerToolCallMetadata {
    pub provider_item_id: String,
    pub actions: Vec<ComputerAction>,
    pub pending_safety_checks: Vec<ComputerSafetyCheck>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerToolResultMetadata {
    pub screenshot: ComputerScreenshotRef,
    pub acknowledged_safety_checks: Vec<ComputerSafetyCheck>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComputerScreenshotRef {
    ImageUrl(String),
    FileId(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum OptionalNullableString {
    #[default]
    Missing,
    Null,
    String(String),
}

impl OptionalNullableString {
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ComputerSafetyCheck {
    pub id: String,
    #[serde(skip_serializing_if = "OptionalNullableString::is_missing")]
    pub code: OptionalNullableString,
    #[serde(skip_serializing_if = "OptionalNullableString::is_missing")]
    pub message: OptionalNullableString,
}

/// Validated JSON retains action-owned extras, as Native structuredCloneJSON does.
#[derive(Clone, Debug, PartialEq)]
pub struct ComputerAction(Value);

impl ComputerAction {
    pub fn as_json(&self) -> &Value {
        &self.0
    }
    pub fn into_json(self) -> Value {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderMetadataError;

impl std::fmt::Display for ProviderMetadataError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("computer provider metadata has an unsupported shape")
    }
}
impl std::error::Error for ProviderMetadataError {}

fn object(value: &Value) -> Result<&Map<String, Value>, ProviderMetadataError> {
    value.as_object().ok_or(ProviderMetadataError)
}
fn nonempty(value: Option<&Value>) -> Result<String, ProviderMetadataError> {
    value.and_then(Value::as_str).filter(|text| !text.is_empty()).map(str::to_owned).ok_or(ProviderMetadataError)
}
fn nullable(value: Option<&Value>) -> Result<OptionalNullableString, ProviderMetadataError> {
    match value {
        None => Ok(OptionalNullableString::Missing),
        Some(Value::Null) => Ok(OptionalNullableString::Null),
        Some(Value::String(text)) => Ok(OptionalNullableString::String(text.clone())),
        _ => Err(ProviderMetadataError),
    }
}
fn safety_check(value: &Value) -> Result<ComputerSafetyCheck, ProviderMetadataError> {
    let fields = object(value)?;
    Ok(ComputerSafetyCheck {
        id: nonempty(fields.get("id"))?,
        code: nullable(fields.get("code"))?,
        message: nullable(fields.get("message"))?,
    })
}
fn safety_checks(value: Option<&Value>) -> Result<Vec<ComputerSafetyCheck>, ProviderMetadataError> {
    value.and_then(Value::as_array).ok_or(ProviderMetadataError)?.iter().map(safety_check).collect()
}
fn finite(value: Option<&Value>) -> bool {
    value.and_then(Value::as_f64).is_some_and(f64::is_finite)
}
fn keys(value: Option<&Value>, optional: bool) -> bool {
    match value {
        None => optional,
        Some(Value::Null) => true,
        Some(Value::Array(keys)) => keys.iter().all(Value::is_string),
        _ => false,
    }
}

impl TryFrom<Value> for ComputerAction {
    type Error = ProviderMetadataError;
    fn try_from(raw: Value) -> Result<Self, Self::Error> {
        let fields = object(&raw)?;
        let xy = || finite(fields.get("x")) && finite(fields.get("y"));
        let valid = match fields.get("type").and_then(Value::as_str) {
            Some("click") => {
                matches!(
                    fields.get("button").and_then(Value::as_str),
                    Some("left" | "right" | "wheel" | "back" | "forward")
                ) && xy()
                    && keys(fields.get("keys"), true)
            }
            Some("double_click") => xy() && keys(fields.get("keys"), false),
            Some("drag") => {
                fields.get("path").and_then(Value::as_array).is_some_and(|path| {
                    path.iter().all(|point| {
                        point.as_object().is_some_and(|point| finite(point.get("x")) && finite(point.get("y")))
                    })
                }) && keys(fields.get("keys"), true)
            }
            Some("keypress") => {
                fields.get("keys").and_then(Value::as_array).is_some_and(|keys| keys.iter().all(Value::is_string))
            }
            Some("move") => xy() && keys(fields.get("keys"), true),
            Some("screenshot" | "wait") => true,
            Some("scroll") => {
                xy() && finite(fields.get("scroll_x"))
                    && finite(fields.get("scroll_y"))
                    && keys(fields.get("keys"), true)
            }
            Some("type") => fields.get("text").is_some_and(Value::is_string),
            _ => false,
        };
        if valid { Ok(Self(raw)) } else { Err(ProviderMetadataError) }
    }
}

fn call_fields(value: &Value) -> Result<ComputerToolCallMetadata, ProviderMetadataError> {
    let fields = object(value)?;
    let actions = fields
        .get("actions")
        .and_then(Value::as_array)
        .filter(|actions| !actions.is_empty())
        .ok_or(ProviderMetadataError)?;
    Ok(ComputerToolCallMetadata {
        provider_item_id: nonempty(fields.get("providerItemId"))?,
        actions: actions.iter().cloned().map(ComputerAction::try_from).collect::<Result<_, _>>()?,
        pending_safety_checks: safety_checks(fields.get("pendingSafetyChecks"))?,
    })
}
fn screenshot(value: &Value) -> Result<ComputerScreenshotRef, ProviderMetadataError> {
    let fields = object(value)?;
    if fields.get("type").and_then(Value::as_str) != Some("computer_screenshot")
        || fields.contains_key("image_url") == fields.contains_key("file_id")
    {
        return Err(ProviderMetadataError);
    }
    if fields.contains_key("image_url") {
        Ok(ComputerScreenshotRef::ImageUrl(nonempty(fields.get("image_url"))?))
    } else {
        Ok(ComputerScreenshotRef::FileId(nonempty(fields.get("file_id"))?))
    }
}
fn result_fields(value: &Value) -> Result<ComputerToolResultMetadata, ProviderMetadataError> {
    let fields = object(value)?;
    Ok(ComputerToolResultMetadata {
        screenshot: screenshot(fields.get("screenshot").ok_or(ProviderMetadataError)?)?,
        acknowledged_safety_checks: safety_checks(fields.get("acknowledgedSafetyChecks"))?,
    })
}

pub fn snapshot_tool_call_provider_metadata(value: Option<&Value>) -> Option<ToolCallProviderMetadata> {
    let value = value?;
    if value.get("type").and_then(Value::as_str) != Some("computer") {
        return None;
    }
    call_fields(value).ok().map(ToolCallProviderMetadata::Computer)
}
pub fn snapshot_tool_result_provider_metadata(
    value: Option<&Value>,
) -> Result<Option<ToolResultProviderMetadata>, ProviderMetadataError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.get("type").and_then(Value::as_str) != Some("computer") {
        return Err(ProviderMetadataError);
    }
    result_fields(value).map(ToolResultProviderMetadata::Computer).map(Some)
}

impl Serialize for OptionalNullableString {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::String(text) => serializer.serialize_str(text),
            _ => serializer.serialize_none(),
        }
    }
}
impl Serialize for ComputerAction {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}
impl Serialize for ComputerScreenshotRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::ImageUrl(url) => serde_json::json!({"type":"computer_screenshot","image_url":url}),
            Self::FileId(id) => serde_json::json!({"type":"computer_screenshot","file_id":id}),
        }
        .serialize(serializer)
    }
}

macro_rules! deserialize_snapshot {
    ($name:ty, $snapshot:expr) => {
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = Value::deserialize(deserializer)?;
                ($snapshot)(&value).map_err(serde::de::Error::custom)
            }
        }
    };
}
deserialize_snapshot!(ComputerToolCallMetadata, call_fields);
deserialize_snapshot!(ComputerToolResultMetadata, result_fields);
deserialize_snapshot!(ComputerSafetyCheck, safety_check);
deserialize_snapshot!(ComputerScreenshotRef, screenshot);
deserialize_snapshot!(ComputerAction, |value: &Value| ComputerAction::try_from(value.clone()));
deserialize_snapshot!(OptionalNullableString, |value: &Value| nullable(Some(value)));
