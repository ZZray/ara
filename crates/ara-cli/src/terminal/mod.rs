//! Fixed OMP 596f2da terminal presentation for the reference Host.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
//! Inputs are admitted, owned display observations. The terminal never owns
//! context, journal, permission, task acceptance, or a tool effect.
pub mod box_component;
pub mod cache_marker;
pub mod diff;
pub mod driver;
pub(crate) mod download_hud;
pub mod edit_renderer;
pub mod evaluation;
pub mod event_controller;
pub(crate) mod graph_renderer;
pub mod highlight;
pub mod hyperlink;
pub mod input;
pub(crate) mod native_view;
pub(crate) mod tsp_wire;
pub(crate) mod tsp_backend;
pub mod markdown;
pub mod markdown_capabilities;
pub mod markdown_lexer;
pub mod markdown_math;
pub mod markdown_stream;
pub mod output_block;
pub mod render_utils;
pub mod session_observation;
pub mod summary_message;
#[cfg(test)]
mod tests;
pub mod text;
pub mod theme;
pub mod tool_execution;
pub mod transcript;
pub mod types;
mod unicode;
pub mod write_renderer;

pub use driver::{TerminalDriver, report_diagnostic};
pub use types::{EditRenderArgs, EditRenderContext, FrameContext, RenderOptions};

pub(crate) mod xdev_renderer;

pub(crate) mod github_output_block;
pub(crate) mod github_renderer;

pub(crate) mod ast_edit_renderer;

#[cfg(test)]
mod edit_upgrade_tests;
