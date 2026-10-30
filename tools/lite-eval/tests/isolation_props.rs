// Feature: ara-lite-core-capability, Property 5: 隔离检查发现注入的泄漏通道
//! For a random workspace tree with randomly injected leak channels (a
//! directory link, a hard link, a copy of an Oracle file, the workspace
//! nested in the Oracle directory, Oracle paths or feature strings in the
//! task text, system prompt, a tool description or the tool environment),
//! `assert_isolated` reports exactly the injected channels with their kind
//! and location, and nothing when none was injected.

mod support;

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use lite_eval::workspace::{assert_isolated, canonical, Exposure, IsolationProblem, Workspace};
use proptest::prelude::*;

const FEATURES: [&str; 3] = ["feat-alpha-1", "feat-beta-22", "feat-gamma-333"];
const DIRS: [&str; 4] = ["", "src/", "src/deep/", "include/"];
const CHANNELS: [&str; 4] = ["text:task", "text:system", "text:tool:ws_read_file", "env:TOOL_ENV"];

fn config() -> ProptestConfig {
    ProptestConfig { failure_persistence: None, ..ProptestConfig::with_cases(256) }
}

/// Oracle file and sealed registration outside the workspace, plus an
/// unrelated directory used as link and hard-link target.
struct Layout {
    base: PathBuf,
    oracle: PathBuf,
    sealed: PathBuf,
    outside: PathBuf,
}

fn layout(dir: &Path) -> Layout {
    let base = canonical(dir).unwrap();
    let oracle = base.join("oracle").join("oracle.py");
    let sealed = base.join("private").join("task-set.sealed.json");
    let outside = base.join("outside");
    fs::create_dir_all(oracle.parent().unwrap()).unwrap();
    fs::create_dir_all(sealed.parent().unwrap()).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(&oracle, format!("CASES = {FEATURES:?}\n")).unwrap();
    fs::write(&sealed, format!("{{\"features\":{FEATURES:?}}}\n")).unwrap();
    fs::write(outside.join("peer.txt"), b"peer-content\n").unwrap();
    Layout { base, oracle, sealed, outside }
}

fn workspace_at(root: &Path) -> Workspace {
    for dir in DIRS {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
    Workspace { root: canonical(root).unwrap(), private: root.with_file_name("host-private") }
}

#[derive(Clone, Debug)]
struct Channel {
    filler: String,
    /// 0: Oracle file with `\`, 1: with `/`, 2: Oracle directory upper-cased,
    /// 3: sealed registration path.
    path_form: Option<usize>,
    feature: Option<usize>,
}

fn channel() -> impl Strategy<Value = Channel> {
    ("[a-z ]{0,12}", prop::option::weighted(0.25, 0usize..4), prop::option::weighted(0.25, 0..FEATURES.len()))
        .prop_map(|(filler, path_form, feature)| Channel { filler, path_form, feature })
}

fn files() -> impl Strategy<Value = BTreeMap<String, String>> {
    prop::collection::btree_map(
        (0..DIRS.len(), "[a-z]{1,6}").prop_map(|(dir, name)| format!("{}{name}.txt", DIRS[dir])),
        "[a-z]{0,16}",
        0..6,
    )
}

fn path_form(layout: &Layout, form: usize) -> String {
    let oracle = canonical(&layout.oracle).unwrap().to_string_lossy().into_owned();
    match form {
        0 => oracle.replace('/', "\\"),
        1 => oracle.replace('\\', "/"),
        2 => canonical(layout.oracle.parent().unwrap()).unwrap().to_string_lossy().to_uppercase(),
        _ => canonical(&layout.sealed).unwrap().to_string_lossy().into_owned(),
    }
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn injected_channels_are_reported_exactly(
        files in files(),
        nested in prop::bool::weighted(0.2),
        link in prop::option::weighted(0.3, 0..DIRS.len()),
        hard in prop::option::weighted(0.3, 0..DIRS.len()),
        copy in prop::option::weighted(0.3, (0..DIRS.len(), prop::bool::ANY)),
        channels in prop::collection::vec(channel(), CHANNELS.len()),
    ) {
        let dir = tempfile::tempdir().unwrap();
        let layout = layout(dir.path());
        let root = if nested { layout.base.join("oracle").join("ws") } else { layout.base.join("ws") };
        let workspace = workspace_at(&root);
        for (relative, content) in &files {
            fs::write(workspace.root.join(relative), content).unwrap();
        }
        let mut expected = Vec::new();
        if nested {
            expected.push(IsolationProblem::Nested {
                workspace: workspace.root.clone(),
                oracle_dir: canonical(layout.oracle.parent().unwrap()).unwrap(),
            });
        }
        if let Some(dir) = link {
            let path = format!("{}inj-link", DIRS[dir]);
            support::dir_link(&workspace.root.join(&path), &layout.outside).unwrap();
            expected.push(IsolationProblem::Link { path });
        }
        if let Some(dir) = hard {
            let path = format!("{}inj-hard.txt", DIRS[dir]);
            fs::hard_link(layout.outside.join("peer.txt"), workspace.root.join(&path)).unwrap();
            expected.push(IsolationProblem::HardLink { path, links: 2 });
        }
        if let Some((dir, registration)) = copy {
            let path = format!("{}inj-oracle.py", DIRS[dir]);
            let source = if registration { &layout.sealed } else { &layout.oracle };
            fs::copy(source, workspace.root.join(&path)).unwrap();
            expected.push(IsolationProblem::OracleCopy { path });
        }
        let mut texts = Vec::new();
        for (label, channel) in CHANNELS.iter().zip(&channels) {
            let mut text = channel.filler.clone();
            if let Some(form) = channel.path_form {
                text = format!("{text} see {} now", path_form(&layout, form));
                expected.push(IsolationProblem::OraclePathExposed { channel: (*label).to_owned() });
            }
            if let Some(feature) = channel.feature {
                text = format!("{text} {} end", FEATURES[feature]);
                expected.push(IsolationProblem::FeatureExposed { channel: (*label).to_owned(), feature: feature + 1 });
            }
            texts.push(text);
        }
        let exposure = Exposure {
            texts: CHANNELS[..3]
                .iter()
                .zip(&texts)
                .map(|(label, text)| (label.trim_start_matches("text:").to_owned(), text.clone()))
                .collect(),
            environment: vec![("TOOL_ENV".to_owned(), texts[3].clone())],
        };
        expected.sort();
        let features: Vec<String> = FEATURES.iter().map(|feature| (*feature).to_owned()).collect();
        let problems =
            assert_isolated(&workspace, &[layout.oracle.clone(), layout.sealed.clone()], &features, &exposure).unwrap();
        prop_assert_eq!(problems.is_empty(), expected.is_empty());
        prop_assert_eq!(&problems, &expected);
        for problem in &problems {
            let report = serde_json::to_string(problem).unwrap();
            prop_assert!(FEATURES.iter().all(|feature| !report.contains(feature)), "{}", report);
        }
    }
}

/// A file symbolic link must be found too. On Windows creating one needs
/// `SeCreateSymbolicLinkPrivilege` or Developer Mode; without it this test
/// fails with the OS error instead of passing silently.
#[test]
fn file_symlink_is_detected() {
    let dir = tempfile::tempdir().unwrap();
    let layout = layout(dir.path());
    let workspace = workspace_at(&layout.base.join("ws"));
    fs::write(workspace.root.join("a.txt"), b"a\n").unwrap();
    let link = workspace.root.join("src").join("inj-symlink.py");
    if let Err(error) = support::file_symlink(&link, &layout.oracle) {
        panic!("file symlink privilege unavailable: {error}");
    }
    let features: Vec<String> = FEATURES.iter().map(|feature| (*feature).to_owned()).collect();
    let problems =
        assert_isolated(&workspace, &[layout.oracle.clone(), layout.sealed], &features, &Exposure::default()).unwrap();
    assert_eq!(problems, vec![IsolationProblem::Link { path: "src/inj-symlink.py".into() }]);
}
