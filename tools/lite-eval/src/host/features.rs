//! P2 switches of the Capability_Test_Host (design “Capability_Test_Host”).
//!
//! Every switch is off in v1, the configuration of the B0 baseline. Each P2
//! sub-item turns its own switch on in its change, so a Run_Manifest's
//! `capability_host_features` names exactly the sub-items in effect.

use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Features {
    /// P2-a: Host output ceiling handed to Core (`ModelHost::output_ceiling`).
    pub output_ceiling: Option<u32>,
    /// P2-a: closed Length stops go to Host review.
    pub length_stop_review: bool,
    /// P2-b: register the public-check completion gate.
    pub completion_gate: bool,
    /// P2-c: Stuck_Detector thresholds set by the Host.
    pub stuck_thresholds: bool,
    /// P2-d: post-compaction refresh limit.
    pub refresh_limit: Option<usize>,
    /// P2-e: register `ws_edit_file` and restrict `ws_write_file` to new files.
    pub edit_file: bool,
}

impl Features {
    /// The v1 (B0) configuration: everything off.
    pub const V1: Features = Features {
        output_ceiling: None,
        length_stop_review: false,
        completion_gate: false,
        stuck_thresholds: false,
        refresh_limit: None,
        edit_file: false,
    };

    /// P2 sub-items with at least one switch on, for the Run_Manifest.
    pub fn enabled(&self) -> Vec<&'static str> {
        [
            ("P2-a", self.output_ceiling.is_some() || self.length_stop_review),
            ("P2-b", self.completion_gate),
            ("P2-c", self.stuck_thresholds),
            ("P2-d", self.refresh_limit.is_some()),
            ("P2-e", self.edit_file),
        ]
        .into_iter()
        .filter_map(|(item, on)| on.then_some(item))
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_has_every_switch_off() {
        assert_eq!(Features::V1, Features::default());
        assert!(Features::V1.enabled().is_empty());
        assert_eq!(Features { edit_file: true, output_ceiling: Some(1), ..Features::V1 }.enabled(), ["P2-a", "P2-e"]);
    }
}
