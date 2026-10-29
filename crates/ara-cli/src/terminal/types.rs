//! Fixed OMP 596f2da display contracts; no Graph handles escape event admission.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use ara_rpc::WireString;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Operation {
    Create,
    Delete,
    #[default]
    Update,
}
impl Operation {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "create" => Some(Self::Create),
            "delete" => Some(Self::Delete),
            "update" => Some(Self::Update),
            _ => None,
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Self::Create => "Create",
            Self::Delete => "Delete",
            Self::Update => "Edit",
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditEntry {
    pub path: Option<WireString>,
    pub rename: Option<WireString>,
    pub move_to: Option<WireString>,
    pub op: Option<Operation>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditRenderArgs {
    pub path: Option<WireString>,
    pub file_path: Option<WireString>,
    pub old_text: Option<WireString>,
    pub new_text: Option<WireString>,
    pub new_string: Option<WireString>,
    pub patch: Option<WireString>,
    pub input: Option<WireString>,
    pub alternate_input: Option<WireString>,
    pub replace_all: Option<bool>,
    pub op: Option<Operation>,
    pub rename: Option<WireString>,
    pub diff: Option<WireString>,
    pub preview_diff: Option<WireString>,
    pub partial_json: Option<WireString>,
    pub edits: Option<Vec<EditEntry>>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diagnostics {
    pub errored: bool,
    pub summary: WireText,
    pub messages: Vec<WireString>,
}
// Wrapper supplies a true empty UTF16 default without changing WireString's API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireText(pub WireString);
impl Default for WireText {
    fn default() -> Self {
        Self(WireString::from(""))
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditDetails {
    pub diff: WireText,
    pub first_changed_line: Option<u32>,
    pub diagnostics: Option<Diagnostics>,
    pub op: Option<Operation>,
    pub move_to: Option<WireString>,
    pub per_file_results: Option<Vec<EditDetails>>,
    pub path: Option<WireString>,
    pub old_text: Option<WireString>,
    pub new_text: Option<WireString>,
    pub snapshots_pruned: Option<bool>,
    pub source_path: Option<WireString>,
    pub is_error: Option<bool>,
    pub error_text: Option<WireString>,
    pub display_error_text: Option<WireString>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditResult {
    pub content: Vec<WireString>,
    pub details: Option<EditDetails>,
    pub is_error: Option<bool>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PerFilePreview {
    pub path: WireText,
    pub diff: Option<WireString>,
    pub first_changed_line: Option<u32>,
    pub error: Option<WireString>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditPreview {
    Diff { diff: WireString, first_changed_line: Option<u32> },
    Error(WireString),
}
pub type RenderDiff = Arc<dyn Fn(&str, Option<&str>) -> String + Send + Sync>;
#[derive(Clone, Default)]
pub struct EditRenderContext {
    pub edit_mode: Option<ara_edit::EditMode>,
    pub edit_diff_preview: Option<EditPreview>,
    pub per_file_diff_preview: Option<Vec<PerFilePreview>>,
    pub edit_streaming_fallback: Option<String>,
    pub render_diff: Option<RenderDiff>,
    /// Host's original same-realm renderContext.renderDiff value. Graph rendering
    /// calls it with the selected diff handle and a fresh {filePath} object in
    /// the effective Session runtime. null/undefined select the native fallback;
    /// other noncallable values fail at the actual Call. None retains render_diff
    /// as the existing Rust Host adapter. Host owns supplying this binding.
    pub render_diff_value: Option<ara_rpc::value_heap::ValueHandle>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderOptions {
    pub expanded: bool,
    pub is_partial: bool,
    pub spinner_frame: Option<usize>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalImageProtocol {
    Kitty,
    Iterm2,
    Sixel,
}
impl TerminalImageProtocol {
    pub const fn as_sequence_prefix(self) -> &'static str {
        match self {
            Self::Kitty => "\x1b_G",
            Self::Iterm2 => "\x1b]1337;File=",
            Self::Sixel => "\x1bPq",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameContext {
    pub columns: usize,
    pub rows: usize,
    pub width_epoch: u64,
    pub hyperlinks: bool,
    pub true_color: bool,
    pub text_sizing: bool,
    pub image_protocol: Option<TerminalImageProtocol>,
}
impl Default for FrameContext {
    fn default() -> Self {
        Self {
            columns: 80,
            rows: 30,
            width_epoch: 0,
            hyperlinks: false,
            true_color: false,
            text_sizing: false,
            image_protocol: None,
        }
    }
}
impl FrameContext {
    pub fn preview_rows(self) -> usize {
        self.rows.max(1).saturating_sub(20).max(6)
    }
}

pub trait Component {
    fn render(&mut self, width: usize, frame: &FrameContext) -> Vec<String>;
    fn invalidate(&mut self);
}
