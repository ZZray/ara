//! Narrow Host acquisition port for pinned OMP b91db66e wasm grammars.
//! Shared Core owns query/results; the Host owns network, locks and cache paths.
use crate::{ToolContext, ToolError};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

#[async_trait]
pub trait GrammarHostPort: Send + Sync {
    /// Return true when at least one requested grammar is now installed.
    /// Acquisition failure is false; caller cancellation remains an error.
    async fn ensure_grammars(&self, languages: &[String], cancel: CancellationToken) -> Result<bool, ToolError>;
}

pub async fn ensure_missing(ctx: &ToolContext, languages: &[String], cancel: CancellationToken) -> Result<bool, ToolError> {
    if languages.is_empty() { return Ok(false); }
    if cancel.is_cancelled() { return Err(ToolError("Aborted: Cancelled".into())); }
    match ctx.grammar_host() {
        Some(host) => host.ensure_grammars(languages, cancel).await,
        None => Ok(false),
    }
}

pub fn missing_grammars_note(languages: &[String]) -> String {
    format!("Skipped {} files: grammar could not be downloaded (offline?); results exclude them.", languages.join(", "))
}
