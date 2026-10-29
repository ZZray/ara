//! CAB/RAR4 reuses the reference Host's installed Date parser/TZ owner.
//! No second timezone cache, CRT path or product state is introduced.
use crate::native_realm::date::{DateHost, SystemDateHost};
use ara_agent::ToolError;
use ara_tools::{ArchiveDatePort, ArchiveLocalDate};

pub(crate) struct SystemArchiveDate;
impl ArchiveDatePort for SystemArchiveDate {
    fn to_utc_ms(&self, fields: ArchiveLocalDate) -> Result<f64, ToolError> {
        let normalized = fields.normalized_local_iso();
        SystemDateHost.parse_ms(&normalized.as_str().into()).map_err(|error| ToolError(error.to_string()))
    }
}
pub fn bind(context: ara_tools::ToolContext) -> ara_tools::ToolContext {
    context.with_archive_date_port(std::sync::Arc::new(SystemArchiveDate))
}
