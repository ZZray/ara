// Feature: ara-lite-core-capability, Property 6: Oracle 目录复制保真且不改动工作区
//! For any workspace tree, the relative path → SHA-256 list recorded when it
//! is copied into a new Oracle directory equals the workspace snapshot (and
//! the copy's own digest), and running a controlled Oracle that rewrites,
//! deletes and adds files in the Oracle directory leaves the workspace
//! snapshot unchanged.

use std::{collections::BTreeMap, fs, time::Duration};

use lite_eval::{
    common::sha256_file,
    digest::tree_digest,
    registry::SealedTask,
    sealed::{copy_workspace, judge, Correctness},
    workspace::{prepare, snapshot},
};
use proptest::prelude::*;

const DIRS: [&str; 4] = ["", "src/", "src/deep/", "include/"];

/// Rewrites every other copied file, deletes the rest, adds one file and
/// scores `passed = files, total = files + 1`.
const FAKE_ORACLE: &str = r#"import argparse, json, pathlib
parser = argparse.ArgumentParser()
parser.add_argument('--artifacts', required=True)
parser.add_argument('--out', required=True)
args = parser.parse_args()
root = pathlib.Path(args.artifacts)
files = sorted(path for path in root.rglob('*') if path.is_file())
for index, path in enumerate(files):
    if index % 2 == 0:
        path.write_bytes(b'rewritten by oracle')
    else:
        path.unlink()
(root / 'oracle-added.txt').write_text('added', encoding='utf-8')
with open(args.out, 'w', encoding='utf-8') as out:
    json.dump({'passed': len(files), 'total': len(files) + 1}, out)
"#;

fn config() -> ProptestConfig {
    ProptestConfig { failure_persistence: None, ..ProptestConfig::with_cases(32) }
}

fn files() -> impl Strategy<Value = BTreeMap<String, Vec<u8>>> {
    let name = prop_oneof![4 => "[a-z]{1,6}\\.(cpp|hpp|txt)", 1 => Just("说明.md".to_owned())];
    prop::collection::btree_map(
        (0..DIRS.len(), name).prop_map(|(dir, name)| format!("{}{name}", DIRS[dir])),
        prop::collection::vec(any::<u8>(), 0..64),
        0..7,
    )
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn oracle_copy_is_faithful_and_workspace_is_untouched(files in files()) {
        let dir = tempfile::tempdir().unwrap();
        let fixture = dir.path().join("fixture");
        fs::create_dir(&fixture).unwrap();
        for (relative, bytes) in &files {
            let path = fixture.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        let run = dir.path().join("run");
        fs::create_dir(&run).unwrap();
        let workspace = prepare(&fixture, &run).unwrap();
        let before = snapshot(&workspace).unwrap();
        prop_assert_eq!(before.len(), files.len());

        // The recorded list equals the snapshot and the copy's own digest.
        let check = dir.path().join("copy-check");
        let copied = copy_workspace(&workspace, &check).unwrap();
        prop_assert_eq!(&copied, &before);
        let on_disk: BTreeMap<_, _> =
            tree_digest(&check).unwrap().entries.into_iter().map(|entry| (entry.path, entry.sha256)).collect();
        prop_assert_eq!(&on_disk, &before);

        // A controlled Oracle mutating its own directory leaves the workspace alone.
        let oracle = dir.path().join("oracle.py");
        fs::write(&oracle, FAKE_ORACLE).unwrap();
        let sealed = SealedTask {
            task_id: "SH1".into(),
            oracle_sha256: sha256_file(&oracle).unwrap(),
            oracle_path: oracle,
            features: vec!["feat-a".into(), "feat-b".into(), "feat-c".into()],
            registered: "2026-10-29".parse().unwrap(),
        };
        let task_dir = run.join("task");
        fs::create_dir(&task_dir).unwrap();
        let outcome = judge(
            &workspace,
            &sealed,
            Vec::new(),
            vec!["run_public_checks".into()],
            &task_dir,
            &[],
            Duration::from_secs(120),
        )
        .unwrap();
        let total = files.len() as u64 + 1;
        prop_assert_eq!(outcome.correctness, Correctness::Judged { passed: files.len() as u64, total });
        prop_assert_eq!(outcome.copied.as_ref(), Some(&before));
        prop_assert_eq!(outcome.workspace_unchanged, Some(true));
        prop_assert!(!outcome.compromised);
        prop_assert_eq!(snapshot(&workspace).unwrap(), before);
        // The Oracle really changed its copy, so the equality above is not vacuous.
        prop_assert!(outcome.oracle_dir.join("oracle-added.txt").is_file());
    }
}
