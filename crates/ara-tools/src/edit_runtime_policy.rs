//! Live EDIT settings port, OMP b91db66e edit/index.ts and edit/settings.ts.
//! The Host owns source configuration/model identity and validation.
use crate::{ToolError, edit::EditSettings};
use ara_edit::EditMode;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditRuntimeSnapshot {
    pub mode: EditMode,
    pub display_mode: EditMode,
    pub settings: EditSettings,
    pub compact_prompt: bool,
}
pub trait EditRuntimePolicy: Send + Sync {
    fn snapshot(&self) -> Result<EditRuntimeSnapshot, ToolError>;
    /// Host identity of the selected-model/configuration reader. Absent
    /// identity is never proof that two independently created readers match.
    fn reader_identity(&self) -> Option<String> {
        None
    }
}
impl<F> EditRuntimePolicy for F
where
    F: Fn() -> Result<EditRuntimeSnapshot, ToolError> + Send + Sync,
{
    fn snapshot(&self) -> Result<EditRuntimeSnapshot, ToolError> {
        self()
    }
}
