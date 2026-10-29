//! Stuck_Detector (requirement 18): pure counting over closed tool rounds.
//!
//! Each configured rule counts independently. The first time a rule reaches
//! its threshold it yields `Hit` (Core injects one correction) and its count
//! restarts; reaching the threshold again yields `NoProgress` (Core stops).
use std::collections::{BTreeMap, BTreeSet, HashSet};

use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StuckRule {
    RepeatedCall,
    RepeatedError,
    RepeatedObservation,
}

/// Host-configured thresholds; an unset threshold disables its rule.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StuckPolicy {
    pub repeated_call: Option<u32>,
    pub repeated_error: Option<u32>,
    pub repeated_observation: Option<u32>,
    /// Tools whose identical successful results count as repeated observations.
    pub observation_tools: BTreeSet<String>,
}

pub const THRESHOLD_RANGE: std::ops::RangeInclusive<u32> = 2..=1000;

impl StuckPolicy {
    /// Every configured threshold must lie in [`THRESHOLD_RANGE`].
    pub fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("repeated_call", self.repeated_call),
            ("repeated_error", self.repeated_error),
            ("repeated_observation", self.repeated_observation),
        ] {
            if let Some(value) = value {
                ensure!(
                    THRESHOLD_RANGE.contains(&value),
                    "stuck_detector {name} threshold {value} is outside 2..=1000"
                );
            }
        }
        Ok(())
    }

    pub fn is_configured(&self) -> bool {
        self.repeated_call.is_some() || self.repeated_error.is_some() || self.repeated_observation.is_some()
    }

    /// Secret-free description for Harness authority hashing and events.
    pub fn declaration(&self) -> Value {
        serde_json::json!({
            "repeated_call": self.repeated_call,
            "repeated_error": self.repeated_error,
            "repeated_observation": self.repeated_observation,
            "observation_tools": self.observation_tools,
        })
    }
}

/// One finished tool call of a closed round.
#[derive(Clone, Debug)]
pub(crate) struct ObservedCall {
    pub native_call_id: String,
    pub tool: String,
    pub arguments: Value,
    pub result: Vec<u8>,
    pub is_error: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Signal {
    Hit { rule: StuckRule, count: u32, threshold: u32, call_ids: Vec<String> },
    NoProgress { rule: StuckRule, count: u32, threshold: u32, call_ids: Vec<String> },
}

#[derive(Default)]
struct RuleState {
    count: u32,
    call_ids: Vec<String>,
    corrected: bool,
}

#[derive(Default)]
pub(crate) struct Detector {
    policy: StuckPolicy,
    call: RuleState,
    last_call: Option<(String, Value, Vec<u8>)>,
    error: RuleState,
    errors: BTreeMap<(String, Vec<u8>), (u32, Vec<String>)>,
    observation: RuleState,
    seen: HashSet<(String, [u8; 32])>,
}

impl Detector {
    pub fn new(policy: &StuckPolicy) -> Self {
        Self { policy: policy.clone(), ..Self::default() }
    }

    /// Feed one closed round in Session order; returns the signals it caused.
    pub fn observe_round(&mut self, round: &[ObservedCall]) -> Vec<Signal> {
        let mut signals = Vec::new();
        for call in round {
            if let Some(threshold) = self.policy.repeated_call {
                let key = (call.tool.clone(), call.arguments.clone(), call.result.clone());
                if self.last_call.as_ref() == Some(&key) {
                    self.call.count += 1;
                } else {
                    self.last_call = Some(key);
                    self.call.count = 1;
                    self.call.call_ids.clear();
                }
                self.call.call_ids.push(call.native_call_id.clone());
                if self.call.count >= threshold {
                    signals.push(fire(&mut self.call, StuckRule::RepeatedCall, threshold));
                }
            }
            if let (Some(threshold), true) = (self.policy.repeated_error, call.is_error) {
                let entry = self.errors.entry((call.tool.clone(), call.result.clone())).or_default();
                entry.0 += 1;
                entry.1.push(call.native_call_id.clone());
                if entry.0 >= threshold {
                    self.error.count = entry.0;
                    self.error.call_ids = entry.1.clone();
                    self.errors.clear();
                    signals.push(fire(&mut self.error, StuckRule::RepeatedError, threshold));
                }
            }
            if let Some(threshold) = self.policy.repeated_observation {
                if !call.is_error && self.policy.observation_tools.contains(&call.tool) {
                    let digest: [u8; 32] = Sha256::digest(&call.result).into();
                    if !self.seen.insert((call.tool.clone(), digest)) {
                        self.observation.count += 1;
                        self.observation.call_ids.push(call.native_call_id.clone());
                        if self.observation.count >= threshold {
                            signals.push(fire(&mut self.observation, StuckRule::RepeatedObservation, threshold));
                        }
                    }
                }
            }
        }
        signals
    }
}

/// Report a rule at its threshold and restart its count.
fn fire(state: &mut RuleState, rule: StuckRule, threshold: u32) -> Signal {
    let (count, call_ids) = (state.count, std::mem::take(&mut state.call_ids));
    state.count = 0;
    if std::mem::replace(&mut state.corrected, true) {
        Signal::NoProgress { rule, count, threshold, call_ids }
    } else {
        Signal::Hit { rule, count, threshold, call_ids }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(id: &str, tool: &str, arguments: Value, result: &str, is_error: bool) -> ObservedCall {
        ObservedCall { native_call_id: id.into(), tool: tool.into(), arguments, result: result.into(), is_error }
    }

    fn rules(signals: &[Signal]) -> Vec<(bool, StuckRule, Vec<String>)> {
        signals
            .iter()
            .map(|signal| match signal {
                Signal::Hit { rule, call_ids, .. } => (false, *rule, call_ids.clone()),
                Signal::NoProgress { rule, call_ids, .. } => (true, *rule, call_ids.clone()),
            })
            .collect()
    }

    #[test]
    fn repeated_call_hits_then_stops_and_any_other_call_restarts_the_count() {
        let mut detector = Detector::new(&StuckPolicy { repeated_call: Some(2), ..StuckPolicy::default() });
        let same = |id| call(id, "read_file", json!({"path":"a","limit":1}), "x", false);
        assert!(detector.observe_round(&[same("1")]).is_empty());
        // Key order of the arguments does not matter.
        let reordered = call("2", "read_file", json!({"limit":1,"path":"a"}), "x", false);
        assert_eq!(
            rules(&detector.observe_round(&[reordered])),
            vec![(false, StuckRule::RepeatedCall, vec!["1".into(), "2".into()])]
        );
        assert!(detector.observe_round(&[same("3")]).is_empty());
        assert!(detector.observe_round(&[call("4", "list_dir", json!({}), "x", false)]).is_empty());
        assert!(detector.observe_round(&[same("5")]).is_empty());
        assert_eq!(
            rules(&detector.observe_round(&[same("6")])),
            vec![(true, StuckRule::RepeatedCall, vec!["5".into(), "6".into()])]
        );
    }

    #[test]
    fn repeated_errors_accumulate_without_being_consecutive() {
        let mut detector = Detector::new(&StuckPolicy { repeated_error: Some(3), ..StuckPolicy::default() });
        let failed = |id| call(id, "exec", json!({"cmd": id}), "exit 1", true);
        let round = [failed("a"), call("b", "exec", json!({}), "ok", false), failed("c"), failed("d")];
        assert_eq!(
            rules(&detector.observe_round(&round)),
            vec![(false, StuckRule::RepeatedError, vec!["a".into(), "c".into(), "d".into()])]
        );
        let again = [failed("e"), failed("f"), failed("g")];
        assert!(rules(&detector.observe_round(&again))[0].0);
    }

    #[test]
    fn repeated_observations_count_only_listed_tools_and_keep_the_seen_set() {
        let policy = StuckPolicy {
            repeated_observation: Some(2),
            observation_tools: ["read_file".to_owned()].into(),
            ..StuckPolicy::default()
        };
        let mut detector = Detector::new(&policy);
        let read = |id| call(id, "read_file", json!({"path": id}), "same", false);
        assert!(detector.observe_round(&[read("1"), call("x", "list_dir", json!({}), "same", false)]).is_empty());
        assert!(detector.observe_round(&[read("2")]).is_empty());
        assert!(!rules(&detector.observe_round(&[read("3")]))[0].0);
        assert!(detector.observe_round(&[read("4")]).is_empty());
        assert!(rules(&detector.observe_round(&[read("5")]))[0].0);
    }

    #[test]
    fn thresholds_are_validated() {
        for bad in [0, 1, 1001] {
            assert!(StuckPolicy { repeated_call: Some(bad), ..StuckPolicy::default() }.validate().is_err());
        }
        assert!(StuckPolicy { repeated_error: Some(2), ..StuckPolicy::default() }.validate().is_ok());
        assert!(!StuckPolicy::default().is_configured());
    }
}

/// Property 24 (requirements 18.1–18.5, 18.9): `Detector` matches a
/// history-scanning reference model and thresholds outside 2..=1000 are refused.
#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::json;

    /// Reference: each rule keeps the plain list of calls since its last firing.
    struct Reference {
        policy: StuckPolicy,
        /// Consecutive identical calls since the last different call or firing.
        streak: Vec<(String, Value, Vec<u8>, String)>,
        last: Option<(String, Value, Vec<u8>)>,
        /// Failed calls since the last repeated_error firing.
        errors: Vec<(String, Vec<u8>, String)>,
        /// Every successful observation result ever seen; never cleared.
        observed: Vec<(String, Vec<u8>)>,
        repeats: Vec<String>,
        fired: BTreeSet<StuckRule>,
    }

    impl Reference {
        fn signal(&mut self, rule: StuckRule, threshold: u32, call_ids: Vec<String>) -> Signal {
            let count = call_ids.len() as u32;
            if self.fired.insert(rule) {
                Signal::Hit { rule, count, threshold, call_ids }
            } else {
                Signal::NoProgress { rule, count, threshold, call_ids }
            }
        }

        fn observe(&mut self, call: &ObservedCall) -> Vec<Signal> {
            let mut signals = Vec::new();
            if let Some(threshold) = self.policy.repeated_call {
                let key = (call.tool.clone(), call.arguments.clone(), call.result.clone());
                if self.last.as_ref() != Some(&key) {
                    self.streak.clear();
                }
                self.last = Some(key.clone());
                self.streak.push((key.0, key.1, key.2, call.native_call_id.clone()));
                if self.streak.len() as u32 >= threshold {
                    let ids = self.streak.drain(..).map(|entry| entry.3).collect();
                    signals.push(self.signal(StuckRule::RepeatedCall, threshold, ids));
                }
            }
            if let (Some(threshold), true) = (self.policy.repeated_error, call.is_error) {
                self.errors.push((call.tool.clone(), call.result.clone(), call.native_call_id.clone()));
                let same: Vec<String> = self
                    .errors
                    .iter()
                    .filter(|(tool, result, _)| tool == &call.tool && result == &call.result)
                    .map(|entry| entry.2.clone())
                    .collect();
                if same.len() as u32 >= threshold {
                    self.errors.clear();
                    signals.push(self.signal(StuckRule::RepeatedError, threshold, same));
                }
            }
            if let Some(threshold) = self.policy.repeated_observation {
                if !call.is_error && self.policy.observation_tools.contains(&call.tool) {
                    let entry = (call.tool.clone(), call.result.clone());
                    if self.observed.contains(&entry) {
                        self.repeats.push(call.native_call_id.clone());
                        if self.repeats.len() as u32 >= threshold {
                            let ids = std::mem::take(&mut self.repeats);
                            signals.push(self.signal(StuckRule::RepeatedObservation, threshold, ids));
                        }
                    } else {
                        self.observed.push(entry);
                    }
                }
            }
            signals
        }
    }

    fn arb_call() -> impl Strategy<Value = (usize, usize, usize, bool)> {
        (0..3usize, 0..2usize, 0..2usize, any::<bool>())
    }

    fn calls(rounds: &[Vec<(usize, usize, usize, bool)>]) -> Vec<Vec<ObservedCall>> {
        let tools = ["read", "list", "exec"];
        let mut next = 0;
        rounds
            .iter()
            .map(|round| {
                round
                    .iter()
                    .map(|(tool, argument, result, is_error)| {
                        next += 1;
                        ObservedCall {
                            native_call_id: format!("c{next}"),
                            tool: tools[*tool].into(),
                            arguments: json!({"path": argument, "limit": 10}),
                            result: format!("result-{result}").into_bytes(),
                            is_error: *is_error,
                        }
                    })
                    .collect()
            })
            .collect()
    }

    fn threshold() -> impl Strategy<Value = Option<u32>> {
        prop::option::of(2u32..=4)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn validation_accepts_exactly_thresholds_in_range(
            values in prop::array::uniform3(prop::option::of(prop_oneof![0u32..=3, 998u32..=1002, Just(u32::MAX)])),
        ) {
            let policy = StuckPolicy {
                repeated_call: values[0],
                repeated_error: values[1],
                repeated_observation: values[2],
                observation_tools: BTreeSet::new(),
            };
            let valid = values.iter().flatten().all(|value| (2..=1000).contains(value));
            prop_assert_eq!(policy.validate().is_ok(), valid);
            prop_assert_eq!(policy.is_configured(), values.iter().any(Option::is_some));
        }

        #[test]
        fn detector_signals_match_the_reference_model(
            repeated_call in threshold(),
            repeated_error in threshold(),
            repeated_observation in threshold(),
            observation in prop::collection::btree_set(prop_oneof![Just("read"), Just("list"), Just("exec")], 0..3),
            rounds in prop::collection::vec(prop::collection::vec(arb_call(), 1..4), 1..14),
        ) {
            let policy = StuckPolicy {
                repeated_call,
                repeated_error,
                repeated_observation,
                observation_tools: observation.into_iter().map(str::to_owned).collect(),
            };
            prop_assert!(policy.validate().is_ok());
            let mut detector = Detector::new(&policy);
            let mut reference = Reference {
                policy: policy.clone(),
                streak: Vec::new(),
                last: None,
                errors: Vec::new(),
                observed: Vec::new(),
                repeats: Vec::new(),
                fired: BTreeSet::new(),
            };
            for round in calls(&rounds) {
                let expected: Vec<Signal> = round.iter().flat_map(|call| reference.observe(call)).collect();
                prop_assert_eq!(detector.observe_round(&round), expected);
            }
        }
    }
}
