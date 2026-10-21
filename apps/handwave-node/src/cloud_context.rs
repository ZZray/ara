//! HandWave wire carrier compatibility; typed snapshot validation belongs to Core.
use anyhow::Result;
use ara_agent_lite_core::context_memory::{parse_snapshot_carrier, CloudSnapshot};
const HEADER: &str = "HANDWAVE_CLOUD_SNAPSHOT_V1\n";
const SEPARATOR: &str = "\nHANDWAVE_BRIEF\n";
pub fn split_cloud_brief<'a>(brief: &'a str, device: &str) -> Result<(&'a str, Option<CloudSnapshot>)> {
    parse_snapshot_carrier(brief, device, HEADER, SEPARATOR)
}
pub fn follow_brief(brief: &str) -> String {
    match brief.strip_prefix(HEADER) {
        Some(framed) => framed.split_once(SEPARATOR).map_or("", |(_, legacy)| legacy).to_owned(),
        None => brief.to_owned(),
    }
}
