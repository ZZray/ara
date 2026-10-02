//! Neutral fixed-OMP snapcompact DTOs. This crate never owns a Session or route.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    pub data: String,
    pub mime_type: String,
    pub cols: f64,
    pub rows: f64,
    pub chars: f64,
    /// Upstream archive parsing retains optional and unknown frame fields.
    #[serde(flatten)]
    pub metadata: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Archive {
    pub frames: Vec<Frame>,
    pub total_chars: f64,
    pub truncated_chars: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_tail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum HistoryBlock {
    Text {
        text: String,
    },
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<Value>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Shape {
    pub font: String,
    pub cell_width: f64,
    pub cell_height: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stretch: Option<bool>,
    pub variant: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopword_dim: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub columns: Option<u32>,
    pub line_repeat: f64,
    pub frame_size: f64,
    pub frame_token_estimate: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_detail: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShapeTarget {
    pub api: Option<String>,
    pub id: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IdealShape {
    pub variant: &'static str,
    pub frame_size: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Geometry {
    pub cols: usize,
    pub rows: usize,
    pub capacity: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderedFrame {
    pub data: String,
    pub cols: usize,
    pub rows: usize,
    pub chars: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileOperations {
    pub read: BTreeSet<String>,
    pub written: BTreeSet<String>,
    pub edited: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionDetails {
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompactionPreparation {
    pub first_kept_entry_id: String,
    /// Already converted OMP model messages. Hosts retain raw entry provenance.
    pub messages_to_summarize: Vec<Value>,
    pub turn_prefix_messages: Vec<Value>,
    pub tokens_before: f64,
    pub previous_summary: Option<String>,
    pub previous_preserve_data: Option<Value>,
    pub file_ops: FileOperations,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionResult {
    pub summary: String,
    pub short_summary: Option<String>,
    pub first_kept_entry_id: String,
    pub tokens_before: f64,
    pub details: Option<CompactionDetails>,
    pub preserve_data: Option<Value>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SerializeOptions {
    pub tool_result_max_chars: f64,
    pub tool_arg_max_chars: f64,
    pub tool_call_max_chars: f64,
    pub truncate_head_ratio: f64,
    pub dim_tool_results: bool,
    pub include_thinking: bool,
}

impl Default for SerializeOptions {
    fn default() -> Self {
        Self {
            tool_result_max_chars: 2000.0,
            tool_arg_max_chars: 500.0,
            tool_call_max_chars: 2000.0,
            truncate_head_ratio: 0.6,
            dim_tool_results: true,
            include_thinking: true,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CompactionOptions {
    pub serialize: SerializeOptions,
    pub model: Option<ShapeTarget>,
    pub shape: Option<Shape>,
    pub frame_size: Option<u32>,
    pub max_frames: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderManyOptions {
    pub shape: Option<Shape>,
    pub model: Option<ShapeTarget>,
    pub frame_size: Option<u32>,
    pub max_frames: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NormalizeOptions {
    pub font: Option<String>,
    pub shape: Option<Shape>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Renderability {
    pub is_safe: bool,
    pub unrenderable_ratio: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HistoryBlockOptions {
    pub max_frame_data_bytes: Option<usize>,
}
