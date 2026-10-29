//! Stage record examples: successful Runs are evidence awaiting review, a
//! Task without a Host conclusion is unaccepted even when everything passed,
//! and every evidence line carries its verification kind.

use std::fs;

use lite_eval::stage::{
    build, evidence_exists, render, sub_items, EvidenceClaim, RunEvidence, TaskVerdict, Verification,
};

#[test]
fn successful_runs_await_review_and_tasks_need_a_host_conclusion() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("real.json"), b"{}").unwrap();
    fs::write(dir.path().join("empty.json"), b"").unwrap();
    fs::create_dir(dir.path().join("empty-dir")).unwrap();

    let mut items = sub_items();
    items[4].claims.push(EvidenceClaim {
        requirement: 19,
        path: "real.json".into(),
        verification: Verification::RealProcess,
    });
    items[5].claims.push(EvidenceClaim {
        requirement: 14,
        path: "empty.json".into(),
        verification: Verification::RealProcess,
    });
    items[6].claims.push(EvidenceClaim {
        requirement: 16,
        path: "empty-dir".into(),
        verification: Verification::RealModelTrial,
    });
    items[11].claims.push(EvidenceClaim {
        requirement: 33,
        path: "real.json".into(),
        verification: Verification::ControlledTest,
    });

    let runs = vec![RunEvidence { run_id: "run-1".into(), task_id: "S3".into() }];
    let tasks = vec![
        TaskVerdict { task_id: "S3".into(), host_accepted: None },
        TaskVerdict { task_id: "S1".into(), host_accepted: Some(true) },
    ];
    let root = dir.path().to_path_buf();
    let record = build("P1", items, &|path| evidence_exists(&root, path), runs, tasks);

    let names: Vec<_> = record.covered.iter().map(|item| item.name.as_str()).collect();
    assert_eq!(names, ["压缩", "回归测试"]);
    assert!(record.uncovered.iter().any(|item| item.name == "多次续写" && item.reason.contains("empty.json")));
    assert!(record.uncovered.iter().any(|item| item.name == "长度截断" && item.reason.contains("empty-dir")));
    assert!(record.goals.iter().all(|goal| !goal.overall_accepted), "every goal still has uncovered items");

    let text = render(&record);
    assert!(text.contains("Run `run-1`（Task `S3`）：待审查证据"), "{text}");
    assert!(text.contains("Task `S3`：未验收（无 Host 结论）"), "{text}");
    assert!(text.contains("Task `S1`：Host 已验收"), "{text}");
    assert!(text.contains("| 2 | 压缩 | 19 | `real.json` | 真实进程 |"), "{text}");
    assert!(text.contains("| 4 | 回归测试 | 33 | `real.json` | 受控测试 |"), "{text}");
    // Every covered evidence line names its verification kind.
    let table_rows = text.lines().filter(|line| line.starts_with("| ") && line.contains('`')).count();
    assert_eq!(table_rows, 2);
}

#[test]
fn a_goal_with_only_controlled_evidence_is_not_accepted() {
    let mut items: Vec<_> = sub_items().into_iter().filter(|item| item.goal == 3).collect();
    for item in &mut items {
        item.claims.push(EvidenceClaim {
            requirement: 27,
            path: "x".into(),
            verification: Verification::ControlledTest,
        });
    }
    let record = build("P4", items.clone(), &|_| true, Vec::new(), Vec::new());
    assert!(record.uncovered.is_empty());
    assert!(!record.goals[0].overall_accepted, "controlled tests alone never complete a goal");
    items[0].claims[0].verification = Verification::RealProcess;
    let record = build("P4", items, &|_| true, Vec::new(), Vec::new());
    assert!(record.goals[0].overall_accepted);
}
