//! P2 switches of the Capability_Test_Host (design “Capability_Test_Host”).
//!
//! Every switch is off in [`Features::V1`], the configuration of the B0
//! baseline. Each P2 sub-item owns its own switches, so a configuration can
//! turn on any combination; [`Features::through`] gives the cumulative
//! configurations of the P2 comparisons (P2-a, then P2-a and P2-b, ...).
//! A Run_Manifest's `capability_host_features` names exactly the sub-items
//! in effect ([`Features::enabled`]).

use std::collections::BTreeSet;

use anyhow::{bail, Result};
use ara_agent_lite_harness::{
    core::{
        compaction_refresh::{FileToolKind, RefreshPolicy},
        stuck_detector::StuckPolicy,
    },
    LengthStopReviewPolicy, TaskOptions,
};
use serde::Serialize;

/// P2 sub-items in comparison order (the names `compare.rs` expects).
pub const P2_ITEMS: [&str; 5] = ["P2-a", "P2-b", "P2-c", "P2-d", "P2-e"];

/// P2-a: Host output ceiling, equal to the Run_Policy `generation.max_output`
/// so the policy value can take effect above the builtin 8192 (design
/// “Run_Policy”); the model's declared `max_tokens` still bounds it.
pub const OUTPUT_CEILING: u32 = 32 * 1024;

/// P2-c: the read-only `ws_*` tools whose repeated identical results count
/// as repeated observations (18.3).
pub const OBSERVATION_TOOLS: [&str; 3] = ["ws_read_file", "ws_list_dir", "ws_search_files"];

/// Stuck_Detector thresholds of the three rules (each in 2..=1000).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct StuckThresholds {
    pub repeated_call: u32,
    pub repeated_error: u32,
    pub repeated_observation: u32,
}

/// P2-c thresholds: three identical calls, errors or observations in a Run.
pub const STUCK_THRESHOLDS: StuckThresholds =
    StuckThresholds { repeated_call: 3, repeated_error: 3, repeated_observation: 3 };

/// Compaction_Refresh limits (Core allows at most 65536 bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct RefreshLimits {
    pub max_files: usize,
    pub max_bytes: usize,
}

/// P2-d limits: the 6 most recent files within 48 KiB.
pub const REFRESH_LIMITS: RefreshLimits = RefreshLimits { max_files: 6, max_bytes: 48 * 1024 };

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Features {
    /// P2-a: Host output ceiling handed to Core (`ModelHost::output_ceiling`).
    pub output_ceiling: Option<u32>,
    /// P2-a: closed Length stops go to Host review.
    pub length_stop_review: bool,
    /// P2-b: register the public-check completion gate.
    pub completion_gate: bool,
    /// P2-c: Stuck_Detector thresholds set by the Host.
    pub stuck: Option<StuckThresholds>,
    /// P2-d: post-compaction refresh limits.
    pub refresh: Option<RefreshLimits>,
    /// P2-e: register `ws_edit_file` and restrict `ws_write_file` to new files.
    pub edit_file: bool,
}

impl Features {
    /// The v1 (B0) configuration: everything off.
    pub const V1: Features = Features {
        output_ceiling: None,
        length_stop_review: false,
        completion_gate: false,
        stuck: None,
        refresh: None,
        edit_file: false,
    };

    /// Every P2 sub-item on: the configuration after P2-e.
    pub const CURRENT: Features = Features {
        output_ceiling: Some(OUTPUT_CEILING),
        length_stop_review: true,
        completion_gate: true,
        stuck: Some(STUCK_THRESHOLDS),
        refresh: Some(REFRESH_LIMITS),
        edit_file: true,
    };

    /// `self` with the switches of one P2 sub-item turned on.
    pub fn with(self, item: &str) -> Result<Features> {
        let current = Features::CURRENT;
        Ok(match item {
            "P2-a" => Features {
                output_ceiling: current.output_ceiling,
                length_stop_review: current.length_stop_review,
                ..self
            },
            "P2-b" => Features { completion_gate: current.completion_gate, ..self },
            "P2-c" => Features { stuck: current.stuck, ..self },
            "P2-d" => Features { refresh: current.refresh, ..self },
            "P2-e" => Features { edit_file: current.edit_file, ..self },
            other => bail!("unknown P2 item {other:?}; expected one of {P2_ITEMS:?}"),
        })
    }

    /// The cumulative configuration of a P2 comparison: every sub-item up to
    /// and including `item` on, the later ones off.
    pub fn through(item: &str) -> Result<Features> {
        let Some(last) = P2_ITEMS.iter().position(|name| *name == item) else {
            bail!("unknown P2 item {item:?}; expected one of {P2_ITEMS:?}");
        };
        P2_ITEMS[..=last].iter().try_fold(Features::V1, |features, name| features.with(name))
    }

    /// P2 sub-items with at least one switch on, for the Run_Manifest.
    pub fn enabled(&self) -> Vec<&'static str> {
        [
            self.output_ceiling.is_some() || self.length_stop_review,
            self.completion_gate,
            self.stuck.is_some(),
            self.refresh.is_some(),
            self.edit_file,
        ]
        .into_iter()
        .zip(P2_ITEMS)
        .filter_map(|(on, item)| on.then_some(item))
        .collect()
    }

    /// Harness options of a Task: P2-a sends closed Length stops to review.
    pub fn task_options(&self) -> TaskOptions {
        TaskOptions {
            length_stop_review: if self.length_stop_review {
                LengthStopReviewPolicy::ReviewClosedLength
            } else {
                LengthStopReviewPolicy::Reject
            },
            ..TaskOptions::default()
        }
    }

    /// P2-c Core policy, `None` when the switch is off.
    pub fn stuck_policy(&self) -> Option<StuckPolicy> {
        self.stuck.map(|thresholds| StuckPolicy {
            repeated_call: Some(thresholds.repeated_call),
            repeated_error: Some(thresholds.repeated_error),
            repeated_observation: Some(thresholds.repeated_observation),
            observation_tools: OBSERVATION_TOOLS.iter().map(|name| (*name).to_owned()).collect::<BTreeSet<_>>(),
        })
    }

    /// P2-d Core policy over the `ws_*` file tools (the Core builtins are not
    /// registered by this Host), `None` when the switch is off.
    pub fn refresh_policy(&self) -> Option<RefreshPolicy> {
        self.refresh.map(|limits| {
            let mut file_tools = vec![("ws_read_file", FileToolKind::Read), ("ws_write_file", FileToolKind::Write)];
            if self.edit_file {
                file_tools.push(("ws_edit_file", FileToolKind::Edit));
            }
            RefreshPolicy {
                file_tools: file_tools.into_iter().map(|(name, kind)| (name.to_owned(), kind)).collect(),
                ..RefreshPolicy::new(limits.max_files, limits.max_bytes)
            }
        })
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
        assert_eq!(Features::V1.task_options(), TaskOptions::default());
        assert!(Features::V1.stuck_policy().is_none() && Features::V1.refresh_policy().is_none());
    }

    #[test]
    fn items_switch_on_individually_and_cumulatively() {
        for (index, item) in P2_ITEMS.iter().enumerate() {
            assert_eq!(Features::V1.with(item).unwrap().enabled(), [*item]);
            assert_eq!(Features::through(item).unwrap().enabled(), P2_ITEMS[..=index]);
        }
        assert_eq!(Features::through("P2-e").unwrap(), Features::CURRENT);
        assert!(Features::through("P2-f").is_err() && Features::V1.with("P2").is_err());
    }

    #[test]
    fn current_policies_are_valid_core_policies() {
        let current = Features::CURRENT;
        assert_eq!(current.task_options().length_stop_review, LengthStopReviewPolicy::ReviewClosedLength);
        let stuck = current.stuck_policy().unwrap();
        stuck.validate().unwrap();
        assert!(stuck.is_configured());
        assert_eq!(stuck.observation_tools.iter().map(String::as_str).collect::<Vec<_>>(), {
            let mut tools = OBSERVATION_TOOLS.to_vec();
            tools.sort();
            tools
        });
        let refresh = current.refresh_policy().unwrap();
        refresh.validate().unwrap();
        assert_eq!(
            refresh.file_tools.keys().map(String::as_str).collect::<Vec<_>>(),
            ["ws_edit_file", "ws_read_file", "ws_write_file"]
        );
        let without_edit = Features { edit_file: false, ..current }.refresh_policy().unwrap();
        assert!(!without_edit.file_tools.contains_key("ws_edit_file"));
    }
}
