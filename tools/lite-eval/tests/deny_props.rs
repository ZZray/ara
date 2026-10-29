// Feature: ara-lite-core-capability, Property 42: 依赖审计比对
//! New dependencies are exactly current − baseline packages; new failures are
//! exactly the current keys not in the baseline, and never appear as known.

use std::collections::BTreeSet;

use lite_eval::deny::{compare, FailureKey};
use proptest::prelude::*;

fn config() -> ProptestConfig {
    ProptestConfig { failure_persistence: None, ..ProptestConfig::with_cases(256) }
}

fn key() -> impl Strategy<Value = FailureKey> {
    (0usize..4, 0u8..5, 0u8..3, 0u8..3).prop_map(|(check, krate, version, rule)| FailureKey {
        check: ["bans", "licenses", "sources", "advisories"][check].to_owned(),
        crate_name: format!("crate{krate}"),
        version: format!("1.0.{version}"),
        rule_or_advisory_id: format!("rule{rule}"),
    })
}

fn package() -> impl Strategy<Value = (String, String)> {
    (0u8..6, 0u8..3).prop_map(|(name, version)| (format!("crate{name}"), format!("1.0.{version}")))
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn comparison_is_exact_set_difference(
        baseline in prop::collection::btree_set(key(), 0..10),
        current in prop::collection::vec(key(), 0..12),
        base_packages in prop::collection::btree_set(package(), 0..10),
        now_packages in prop::collection::btree_set(package(), 0..10),
    ) {
        let result = compare(&baseline, &current, &base_packages, &now_packages);
        let expected_new_deps: Vec<_> = now_packages.difference(&base_packages).cloned().collect();
        prop_assert_eq!(&result.new_dependencies, &expected_new_deps);

        let current_set: BTreeSet<_> = current.iter().cloned().collect();
        let expected_new: Vec<_> = current_set.difference(&baseline).cloned().collect();
        let expected_known: Vec<_> = current_set.intersection(&baseline).cloned().collect();
        prop_assert_eq!(&result.new_failures, &expected_new);
        prop_assert_eq!(&result.known_failures, &expected_known);
        prop_assert!(result.new_failures.iter().all(|key| !result.known_failures.contains(key)));
        for key in &result.new_failures_from_new_dependencies {
            prop_assert!(result.new_failures.contains(key));
            prop_assert!(expected_new_deps.contains(&(key.crate_name.clone(), key.version.clone())));
        }
    }
}
