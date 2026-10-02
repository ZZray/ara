//! Complete local snapcompact port from OMP 596f2da7101178214aa27a753529d15e6b7ad91d.
//! Source: packages/snapcompact and crates/pi-natives/src/snapcompact.rs (MIT).
//! Hosts own source snapshots, cancellation, budgets and Session publication.
mod archive;
mod compact;
mod files;
mod identity;
mod layout;
pub mod native;
mod shape;
mod text;
mod types;
pub use archive::{
    archive_source_text, frame_data_bytes, get_preserved_archive, history_blocks, images, renderability_probe_text,
    strip_preserved_archive,
};
pub use compact::compact;
pub use files::{compute_file_lists, create_file_ops, is_url_scheme_path, upsert_file_operations};
pub use layout::{frames, geometry, render, render_many, wrap};
pub use shape::{
    SHAPE_VARIANT_NAMES, ideal_shape_variant, is_shape, is_shape_variant_name, max_frames_for_data_budget,
    provider_frame_budget, provider_image_budget, resolve_shape, resolve_shape_for_text, shape_variant,
};
pub use text::{dim_stopwords, normalize, scan_renderability, serialize_conversation, strip_dim_markers};
pub use types::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub String);
impl Error {
    pub fn from_reason(reason: impl Into<String>) -> Self {
        Self(reason.into())
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

pub const FRAME_SIZE: usize = 2576;
pub const MAX_FRAMES_DEFAULT: usize = 80;
pub const HQ_EDGE_FRAMES: usize = 3;
pub const FRAME_TOKEN_ESTIMATE: usize = 5024;
pub const FRAME_DATA_BYTES_ESTIMATE: usize = 170_000;
pub const FRAME_DATA_BYTES_BUDGET: usize = 3_000_000;
pub const DEFAULT_PROVIDER_IMAGE_BUDGET: usize = 5;
pub const PRESERVE_KEY: &str = "snapcompact";
pub const TOOL_RESULT_MAX_CHARS: usize = 2000;
pub const TOOL_ARG_MAX_CHARS: usize = 500;
pub const TOOL_CALL_MAX_CHARS: usize = 2000;
pub const TRUNCATE_HEAD_RATIO: f64 = 0.6;
pub const DIM_ON: char = '\u{e}';
pub const DIM_OFF: char = '\u{f}';
pub const NEWLINE_GLYPH: char = '\u{2588}';
