//! Prompt templates for the ARA Agent Core: a Handlebars-compatible engine
//! ([`template`]), the shared prompt helpers, compile cache, `render` and the
//! `format` normalizer ([`prompt`]). Ported from OMP `packages/utils/src/
//! {template,prompt}.ts` at 596f2da7101178214aa27a753529d15e6b7ad91d.

mod js;
pub mod prompt;
pub mod template;

pub use prompt::{FormatOptions, RenderPhase, compile, format, register_helper, register_partial, render};
pub use template::{CompileOptions, Engine, HelperCall, Output, Template, TemplateError, escape_expression};
