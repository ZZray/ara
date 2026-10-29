// Feature: ara-lite-core-capability, Property 43: 阶段验收记录构造
//! Every sub-item lands in exactly one of covered/uncovered; missing or empty
//! evidence moves it to uncovered; a goal with any uncovered item, or with
//! only controlled/demo evidence, is not accepted overall.

use std::collections::BTreeSet;

use lite_eval::stage::{build, EvidenceClaim, SubItem, Verification};
use proptest::prelude::*;

fn config() -> ProptestConfig {
    ProptestConfig { failure_persistence: None, ..ProptestConfig::with_cases(256) }
}

fn verification() -> impl Strategy<Value = Verification> {
    prop_oneof![
        Just(Verification::ControlledTest),
        Just(Verification::Demo),
        Just(Verification::RealProcess),
        Just(Verification::RealModelTrial),
    ]
}

/// A claim plus whether its evidence path "exists".
fn claim() -> impl Strategy<Value = (EvidenceClaim, bool)> {
    (1u8..39, 0u16..1000, verification(), any::<bool>()).prop_map(|(requirement, id, verification, exists)| {
        (EvidenceClaim { requirement, path: format!("evidence/{id}-{exists}.json"), verification }, exists)
    })
}

fn items() -> impl Strategy<Value = Vec<(u8, Vec<(EvidenceClaim, bool)>)>> {
    prop::collection::vec((1u8..5, prop::collection::vec(claim(), 0..4)), 1..12)
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn stage_record_partitions_items_and_judges_goals(raw in items()) {
        let existing: BTreeSet<String> =
            raw.iter().flat_map(|(_, claims)| claims.iter().filter(|(_, ok)| *ok).map(|(c, _)| c.path.clone())).collect();
        let sub_items: Vec<SubItem> = raw
            .iter()
            .enumerate()
            .map(|(index, (goal, claims))| SubItem {
                goal: *goal,
                name: format!("item{index}"),
                claims: claims.iter().map(|(claim, _)| claim.clone()).collect(),
            })
            .collect();
        let record = build("P5", sub_items.clone(), &|path| existing.contains(path), Vec::new(), Vec::new());

        prop_assert_eq!(record.covered.len() + record.uncovered.len(), sub_items.len());
        for item in &sub_items {
            let in_covered = record.covered.iter().filter(|c| c.name == item.name).count();
            let in_uncovered = record.uncovered.iter().filter(|u| u.name == item.name).count();
            prop_assert_eq!(in_covered + in_uncovered, 1, "{} must appear exactly once", item.name);
            let has_good = item.claims.iter().any(|claim| existing.contains(&claim.path));
            prop_assert_eq!(in_covered == 1, has_good);
        }
        for covered in &record.covered {
            prop_assert!(covered.evidence.iter().all(|claim| existing.contains(&claim.path)));
        }
        for goal in &record.goals {
            let any_uncovered = record.uncovered.iter().any(|item| item.goal == goal.goal);
            let real = record.covered.iter().filter(|item| item.goal == goal.goal)
                .flat_map(|item| &item.evidence).any(|claim| claim.verification.counts_for_acceptance());
            prop_assert_eq!(goal.overall_accepted, !any_uncovered && real);
        }
    }
}
